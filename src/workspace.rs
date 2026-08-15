// Copyright 2026 Kotelnikovekb
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://apache.org
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.
use crate::{
    config::RunnerConfig,
    job::{GitPublishMode, WorkspaceSpec},
};
use anyhow::{Context, Result, bail};
use std::{
    fs,
    io::Write,
    path::{Component, Path},
    process::Command,
};
use tempfile::TempDir;
use walkdir::WalkDir;
pub struct PreparedWorkspace {
    pub dir: TempDir,
    #[allow(dead_code)]
    pub container_path: String,
}
pub async fn prepare(spec: &WorkspaceSpec, _c: &RunnerConfig) -> Result<PreparedWorkspace> {
    // Keep the staging directory outside the source workspace. Otherwise a
    // local workspace of `.` would contain its own destination and recurse
    // until macOS reports `File name too long`.
    let dir = tempfile::Builder::new()
        .prefix("omniroute-job-")
        .tempdir()
        .context("create job directory")?;
    match spec {
        WorkspaceSpec::Local { path } => copy_tree(Path::new(path), dir.path())?,
        WorkspaceSpec::ArchiveUrl { url } => download_archive(url, dir.path()).await?,
        WorkspaceSpec::Git {
            clone_url,
            base_branch,
            branch,
            base_sha,
            head_sha,
            username,
            token,
            ..
        } => {
            let output = Command::new("git")
                .args([
                    "-c",
                    &format!("http.extraheader=Authorization: Bearer {token}"),
                    "clone",
                    "--depth",
                    "1",
                    "--branch",
                    branch,
                    "--single-branch",
                    clone_url,
                    ".",
                ])
                .current_dir(dir.path())
                .env("GIT_USERNAME", username)
                .env("GIT_TOKEN", token)
                .env("GIT_TERMINAL_PROMPT", "0")
                .output()
                .context("clone git workspace")?;
            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                bail!("git clone failed: {}", stderr.trim())
            }
            if base_branch != branch {
                let base_refspec =
                    format!("+refs/heads/{base_branch}:refs/remotes/origin/{base_branch}");
                git_with_auth(
                    dir.path(),
                    token,
                    &["fetch", "--depth", "1", "origin", &base_refspec],
                    "fetch base branch",
                )?;
            }
            verify_revision(dir.path(), "HEAD", head_sha.as_deref(), "head_sha")?;
            verify_revision(
                dir.path(),
                &format!("refs/remotes/origin/{base_branch}"),
                base_sha.as_deref(),
                "base_sha",
            )?;
        }
    };
    if dir.path().join(".git").is_dir() {
        exclude_runner_files(dir.path())?;
    }
    Ok(PreparedWorkspace {
        dir,
        container_path: "/workspace".into(),
    })
}

pub fn publish_git(spec: &WorkspaceSpec, dir: &Path) -> Result<()> {
    let WorkspaceSpec::Git {
        branch,
        token,
        commit_message,
        publish_mode,
        ..
    } = spec
    else {
        return Ok(());
    };
    if *publish_mode == GitPublishMode::Disabled {
        return Ok(());
    }
    let status = Command::new("git")
        .args(["config", "user.name", "AI Kodu Runner"])
        .current_dir(dir)
        .status()?;
    if !status.success() {
        bail!("git user configuration failed")
    }
    exclude_runner_files(dir)?;
    let status = Command::new("git")
        .args(["config", "user.email", "runner@ai-kodu.local"])
        .current_dir(dir)
        .status()?;
    if !status.success() {
        bail!("git user configuration failed")
    }
    let status = Command::new("git")
        .args(["add", "-A"])
        .current_dir(dir)
        .status()?;
    if !status.success() {
        bail!("git add failed")
    }
    let status = Command::new("git")
        .args(["diff", "--cached", "--quiet", "--exit-code"])
        .current_dir(dir)
        .status()
        .context("inspect staged git changes")?;
    match status.code() {
        Some(0) if *publish_mode == GitPublishMode::Required => {
            bail!("git publish required changes, but the working tree is clean")
        }
        Some(0) => return Ok(()),
        Some(1) => {}
        _ => bail!("git diff failed"),
    }
    let status = Command::new("git")
        .args(["commit", "-m", commit_message])
        .current_dir(dir)
        .status()?;
    if !status.success() {
        bail!("git commit failed")
    }
    let status = Command::new("git")
        .args([
            "-c",
            &format!("http.extraheader=Authorization: Bearer {token}"),
            "push",
            "origin",
            branch,
        ])
        .current_dir(dir)
        .status()?;
    if !status.success() {
        bail!("git push failed")
    }
    Ok(())
}

fn git_with_auth(dir: &Path, token: &str, args: &[&str], operation: &str) -> Result<()> {
    let output = Command::new("git")
        .args([
            "-c",
            &format!("http.extraheader=Authorization: Bearer {token}"),
        ])
        .args(args)
        .current_dir(dir)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .with_context(|| operation.to_owned())?;
    if !output.status.success() {
        bail!(
            "{operation} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )
    }
    Ok(())
}

fn verify_revision(dir: &Path, revision: &str, expected: Option<&str>, name: &str) -> Result<()> {
    let Some(expected) = expected else {
        return Ok(());
    };
    let output = Command::new("git")
        .args(["rev-parse", revision])
        .current_dir(dir)
        .output()
        .with_context(|| format!("resolve {name}"))?;
    if !output.status.success() {
        bail!("{name} {expected} is not available in the prepared workspace")
    }
    let actual = String::from_utf8_lossy(&output.stdout);
    if actual.trim() != expected {
        bail!(
            "{name} mismatch: expected {expected}, got {}",
            actual.trim()
        )
    }
    Ok(())
}

fn exclude_runner_files(dir: &Path) -> Result<()> {
    let exclude_path = dir.join(".git/info/exclude");
    let mut existing = fs::read_to_string(&exclude_path).unwrap_or_default();
    for pattern in [
        ".cache/",
        ".omniroute/",
        ".dart_tool/",
        ".runner-cache/",
        "build/",
        "node_modules/",
        "prompt.md",
    ] {
        if !existing.lines().any(|line| line.trim() == pattern) {
            if !existing.is_empty() && !existing.ends_with('\n') {
                existing.push('\n');
            }
            existing.push_str(pattern);
            existing.push('\n');
        }
    }
    let mut file = fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(exclude_path)?;
    file.write_all(existing.as_bytes())?;
    Ok(())
}
fn copy_tree(from: &Path, to: &Path) -> Result<()> {
    if !from.is_dir() {
        bail!("workspace is not a directory")
    }
    for e in WalkDir::new(from)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| {
            entry
                .path()
                .strip_prefix(from)
                .map(|rel| !is_derived_path(rel))
                .unwrap_or(false)
        })
    {
        let e = e?;
        let rel = e.path().strip_prefix(from)?;
        if rel.as_os_str().is_empty() {
            continue;
        }
        let out = to.join(rel);
        if e.file_type().is_dir() {
            fs::create_dir_all(&out)?
        } else if e.file_type().is_file() {
            if let Some(p) = out.parent() {
                fs::create_dir_all(p)?
            }
            fs::copy(e.path(), out)?;
        }
    }
    Ok(())
}

/// Directories produced by agents and build tools are never part of the input
/// workspace. Keeping them out prevents OpenCode snapshots from indexing their
/// own database, package caches, and generated dependency trees.
pub(crate) fn is_derived_path(path: &Path) -> bool {
    path.components().any(|component| {
        let Component::Normal(name) = component else {
            return false;
        };
        matches!(
            name.to_str(),
            Some(
                ".cache" | ".omniroute" | ".dart_tool" | ".runner-cache" | "build" | "node_modules"
            )
        )
    })
}
async fn download_archive(url: &str, to: &Path) -> Result<()> {
    let parsed = url::Url::parse(url).context("parse archive URL")?;
    if parsed.scheme() != "https" {
        bail!("archive URL must use HTTPS")
    }
    let bytes = reqwest::get(parsed)
        .await?
        .error_for_status()?
        .bytes()
        .await?;
    let mut ar = tar::Archive::new(std::io::Cursor::new(bytes));
    for item in ar.entries()? {
        let mut e = item?;
        let p = e.path()?.to_path_buf();
        if p.is_absolute()
            || p.components()
                .any(|x| matches!(x, std::path::Component::ParentDir))
        {
            bail!("archive path traversal detected")
        }
        let out = to.join(&p);
        if !out.starts_with(to) {
            bail!("archive escape detected")
        }
        if e.header().entry_type().is_symlink() || e.header().entry_type().is_hard_link() {
            bail!("links are not allowed in archives")
        }
        e.unpack(&out)?;
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;

    fn git(dir: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    }

    fn git_workspace(
        clone_url: String,
        base_sha: Option<String>,
        head_sha: Option<String>,
        publish_mode: GitPublishMode,
    ) -> WorkspaceSpec {
        WorkspaceSpec::Git {
            clone_url,
            base_branch: "main".into(),
            branch: "task/change".into(),
            base_sha,
            head_sha,
            username: "runner".into(),
            token: String::new(),
            commit_message: "runner changes".into(),
            publish_mode,
        }
    }

    #[test]
    fn rejects_escape() {
        assert!("../etc/passwd".split('/').any(|x| x == ".."));
    }

    #[test]
    fn copy_tree_prunes_generated_directories() {
        let source = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();
        fs::create_dir_all(source.path().join("lib")).unwrap();
        fs::create_dir_all(source.path().join("node_modules/pkg")).unwrap();
        fs::create_dir_all(source.path().join("packages/app/.dart_tool")).unwrap();
        fs::write(source.path().join("lib/main.dart"), "void main() {}\n").unwrap();
        fs::write(
            source.path().join("node_modules/pkg/index.js"),
            "generated\n",
        )
        .unwrap();
        fs::write(
            source.path().join("packages/app/.dart_tool/state"),
            "generated\n",
        )
        .unwrap();

        copy_tree(source.path(), destination.path()).unwrap();

        assert!(destination.path().join("lib/main.dart").is_file());
        assert!(!destination.path().join("node_modules").exists());
        assert!(!destination.path().join("packages/app/.dart_tool").exists());
    }

    #[tokio::test]
    async fn prepares_remote_task_branch_and_base_revision() {
        let root = tempfile::tempdir().unwrap();
        let origin = root.path().join("origin.git");
        let source = root.path().join("source");
        fs::create_dir(&source).unwrap();
        git(root.path(), &["init", "--bare", origin.to_str().unwrap()]);
        git(&source, &["init", "-b", "main"]);
        git(&source, &["config", "user.name", "Test"]);
        git(&source, &["config", "user.email", "test@example.com"]);
        fs::write(source.join("README.md"), "base\n").unwrap();
        git(&source, &["add", "README.md"]);
        git(&source, &["commit", "-m", "base"]);
        let base_sha = git(&source, &["rev-parse", "HEAD"]);
        git(&source, &["checkout", "-b", "task/change"]);
        fs::write(source.join("README.md"), "task\n").unwrap();
        git(&source, &["commit", "-am", "task"]);
        let head_sha = git(&source, &["rev-parse", "HEAD"]);
        git(
            &source,
            &["remote", "add", "origin", origin.to_str().unwrap()],
        );
        git(&source, &["push", "origin", "main", "task/change"]);

        let spec = git_workspace(
            origin.to_string_lossy().into_owned(),
            Some(base_sha.clone()),
            Some(head_sha.clone()),
            GitPublishMode::IfChanged,
        );
        let prepared = prepare(&spec, &RunnerConfig::default_local())
            .await
            .unwrap();

        assert_eq!(git(prepared.dir.path(), &["rev-parse", "HEAD"]), head_sha);
        assert_eq!(
            git(
                prepared.dir.path(),
                &["rev-parse", "refs/remotes/origin/main"]
            ),
            base_sha
        );
        assert_eq!(
            fs::read_to_string(prepared.dir.path().join("README.md")).unwrap(),
            "task\n"
        );
    }

    #[test]
    fn publish_if_changed_accepts_clean_workspace() {
        let repo = tempfile::tempdir().unwrap();
        git(repo.path(), &["init", "-b", "task/change"]);
        git(repo.path(), &["config", "user.name", "Test"]);
        git(repo.path(), &["config", "user.email", "test@example.com"]);
        fs::write(repo.path().join("README.md"), "clean\n").unwrap();
        git(repo.path(), &["add", "README.md"]);
        git(repo.path(), &["commit", "-m", "initial"]);
        let spec = git_workspace(String::new(), None, None, GitPublishMode::IfChanged);

        publish_git(&spec, repo.path()).unwrap();
        assert_eq!(git(repo.path(), &["rev-list", "--count", "HEAD"]), "1");
    }

    #[test]
    fn required_publish_rejects_clean_workspace() {
        let repo = tempfile::tempdir().unwrap();
        git(repo.path(), &["init", "-b", "task/change"]);
        let spec = git_workspace(String::new(), None, None, GitPublishMode::Required);

        let error = publish_git(&spec, repo.path()).unwrap_err();
        assert!(error.to_string().contains("required changes"));
    }
}
