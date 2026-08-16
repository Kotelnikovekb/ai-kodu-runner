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
use crate::config::{Limits, RunnerConfig};
use anyhow::{Context, Result, bail};
use futures_util::StreamExt;
use runner_protocol::{GitPublishMode, WorkspaceSpec};
use std::{
    fs,
    io::Write,
    path::{Component, Path},
};
use tempfile::TempDir;
use tokio::process::Command as TokioCommand;
use tokio::time::{Instant, sleep_until};
use tokio_util::sync::CancellationToken;
use walkdir::WalkDir;
pub struct PreparedWorkspace {
    pub dir: TempDir,
    #[allow(dead_code)]
    pub container_path: String,
}
pub async fn prepare(
    spec: &WorkspaceSpec,
    config: &RunnerConfig,
    cancellation: &CancellationToken,
    deadline: Instant,
) -> Result<PreparedWorkspace> {
    // Keep the staging directory outside the source workspace. Otherwise a
    // local workspace of `.` would contain its own destination and recurse
    // until macOS reports `File name too long`.
    let dir = tempfile::Builder::new()
        .prefix("ai-kodu-runner-job-")
        .tempdir()
        .context("create job directory")?;
    match spec {
        WorkspaceSpec::Local { path } => copy_tree(
            Path::new(path),
            dir.path(),
            &config.limits,
            cancellation,
            deadline,
        )?,
        WorkspaceSpec::ArchiveUrl { url } => {
            download_archive(url, dir.path(), &config.limits, cancellation, deadline).await?
        }
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
            let mut command = TokioCommand::new("git");
            command
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
                .kill_on_drop(true);
            let output = run_process(command, cancellation, deadline)
                .await
                .context("clone git workspace")?;
            if !output.status.success() {
                let stderr = redact_git_token(&String::from_utf8_lossy(&output.stderr), token);
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
                    cancellation,
                    deadline,
                )
                .await?;
            }
            verify_revision(
                dir.path(),
                "HEAD",
                head_sha.as_deref(),
                "head_sha",
                cancellation,
                deadline,
            )
            .await?;
            verify_revision(
                dir.path(),
                &format!("refs/remotes/origin/{base_branch}"),
                base_sha.as_deref(),
                "base_sha",
                cancellation,
                deadline,
            )
            .await?;
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

pub async fn publish_git(
    spec: &WorkspaceSpec,
    dir: &Path,
    cancellation: &CancellationToken,
    deadline: Instant,
) -> Result<()> {
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
    run_git_command(
        dir,
        &["config", "user.name", "AI Kodu Runner"],
        "git user configuration",
        cancellation,
        deadline,
    )
    .await?;
    exclude_runner_files(dir)?;
    run_git_command(
        dir,
        &["config", "user.email", "runner@ai-kodu.local"],
        "git user configuration",
        cancellation,
        deadline,
    )
    .await?;
    run_git_command(dir, &["add", "-A"], "git add", cancellation, deadline).await?;
    let diff = run_git_output(
        dir,
        &["diff", "--cached", "--quiet", "--exit-code"],
        "inspect staged git changes",
        cancellation,
        deadline,
    )
    .await?;
    match diff.status.code() {
        Some(0) if *publish_mode == GitPublishMode::Required => {
            bail!("git publish required changes, but the working tree is clean")
        }
        Some(0) => return Ok(()),
        Some(1) => {}
        _ => bail!("git diff failed"),
    }
    run_git_command(
        dir,
        &["commit", "-m", commit_message],
        "git commit",
        cancellation,
        deadline,
    )
    .await?;
    git_with_auth(
        dir,
        token,
        &["push", "origin", branch],
        "git push",
        cancellation,
        deadline,
    )
    .await?;
    Ok(())
}

async fn run_git_output(
    dir: &Path,
    args: &[&str],
    operation: &str,
    cancellation: &CancellationToken,
    deadline: Instant,
) -> Result<std::process::Output> {
    let mut command = TokioCommand::new("git");
    command
        .args(args)
        .current_dir(dir)
        .env("GIT_TERMINAL_PROMPT", "0")
        .kill_on_drop(true);
    run_process(command, cancellation, deadline)
        .await
        .with_context(|| operation.to_owned())
}

async fn run_git_command(
    dir: &Path,
    args: &[&str],
    operation: &str,
    cancellation: &CancellationToken,
    deadline: Instant,
) -> Result<()> {
    let output = run_git_output(dir, args, operation, cancellation, deadline).await?;
    if !output.status.success() {
        bail!("{operation} failed")
    }
    Ok(())
}

async fn git_with_auth(
    dir: &Path,
    token: &str,
    args: &[&str],
    operation: &str,
    cancellation: &CancellationToken,
    deadline: Instant,
) -> Result<()> {
    let mut command = TokioCommand::new("git");
    command
        .args([
            "-c",
            &format!("http.extraheader=Authorization: Bearer {token}"),
        ])
        .args(args)
        .current_dir(dir)
        .env("GIT_TERMINAL_PROMPT", "0")
        .kill_on_drop(true);
    let output = run_process(command, cancellation, deadline)
        .await
        .with_context(|| operation.to_owned())?;
    if !output.status.success() {
        bail!(
            "{operation} failed: {}",
            redact_git_token(&String::from_utf8_lossy(&output.stderr), token).trim()
        )
    }
    Ok(())
}

fn redact_git_token(message: &str, token: &str) -> String {
    if token.is_empty() {
        return message.to_owned();
    }
    message.replace(token, "<redacted-git-token>")
}

async fn verify_revision(
    dir: &Path,
    revision: &str,
    expected: Option<&str>,
    name: &str,
    cancellation: &CancellationToken,
    deadline: Instant,
) -> Result<()> {
    let Some(expected) = expected else {
        return Ok(());
    };
    let mut command = TokioCommand::new("git");
    command
        .args(["rev-parse", revision])
        .current_dir(dir)
        .kill_on_drop(true);
    let output = run_process(command, cancellation, deadline)
        .await
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

async fn run_process(
    mut command: TokioCommand,
    cancellation: &CancellationToken,
    deadline: Instant,
) -> Result<std::process::Output> {
    tokio::select! {
        output = command.output() => Ok(output?),
        _ = cancellation.cancelled() => bail!("workspace preparation cancelled"),
        _ = sleep_until(deadline) => bail!("workspace preparation timed out"),
    }
}

fn exclude_runner_files(dir: &Path) -> Result<()> {
    let exclude_path = dir.join(".git/info/exclude");
    let mut existing = fs::read_to_string(&exclude_path).unwrap_or_default();
    for pattern in [
        ".cache/",
        ".ai-kodu-runner/",
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
fn copy_tree(
    from: &Path,
    to: &Path,
    limits: &Limits,
    cancellation: &CancellationToken,
    deadline: Instant,
) -> Result<()> {
    if !from.is_dir() {
        bail!("workspace is not a directory")
    }
    let mut bytes = 0u64;
    let mut files = 0usize;
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
        ensure_budget(cancellation, deadline)?;
        let e = e?;
        let rel = e.path().strip_prefix(from)?;
        if rel.as_os_str().is_empty() {
            continue;
        }
        let out = to.join(rel);
        if e.file_type().is_dir() {
            fs::create_dir_all(&out)?
        } else if e.file_type().is_file() {
            files = files
                .checked_add(1)
                .ok_or_else(|| anyhow::anyhow!("workspace file count overflow"))?;
            if files > limits.max_workspace_files {
                bail!("workspace file limit exceeded")
            }
            bytes = bytes
                .checked_add(e.metadata()?.len())
                .ok_or_else(|| anyhow::anyhow!("workspace size overflow"))?;
            if bytes > limits.max_workspace_bytes {
                bail!("workspace byte limit exceeded")
            }
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
                ".cache"
                    | ".ai-kodu-runner"
                    | ".dart_tool"
                    | ".runner-cache"
                    | "build"
                    | "node_modules"
            )
        )
    })
}
async fn download_archive(
    url: &str,
    to: &Path,
    limits: &Limits,
    cancellation: &CancellationToken,
    deadline: Instant,
) -> Result<()> {
    let parsed = url::Url::parse(url).context("parse archive URL")?;
    if parsed.scheme() != "https" {
        bail!("archive URL must use HTTPS")
    }
    let response = tokio::select! {
        response = reqwest::get(parsed) => response?.error_for_status()?,
        _ = cancellation.cancelled() => bail!("workspace preparation cancelled"),
        _ = sleep_until(deadline) => bail!("workspace preparation timed out"),
    };
    if let Some(length) = response.content_length()
        && length > limits.max_archive_download_bytes
    {
        bail!("archive download limit exceeded")
    }
    let mut stream = response.bytes_stream();
    let mut bytes = Vec::new();
    while let Some(chunk) = tokio::select! {
        chunk = stream.next() => chunk,
        _ = cancellation.cancelled() => bail!("workspace preparation cancelled"),
        _ = sleep_until(deadline) => bail!("workspace preparation timed out"),
    } {
        let chunk = chunk?;
        if (bytes.len() as u64).saturating_add(chunk.len() as u64)
            > limits.max_archive_download_bytes
        {
            bail!("archive download limit exceeded")
        }
        bytes.extend_from_slice(&chunk);
    }
    let mut ar = tar::Archive::new(std::io::Cursor::new(bytes));
    let mut expanded_bytes = 0u64;
    let mut files = 0usize;
    let mut entries = 0usize;
    for item in ar.entries()? {
        ensure_budget(cancellation, deadline)?;
        let mut e = item?;
        entries = entries
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("archive entry count overflow"))?;
        if entries > limits.max_workspace_files {
            bail!("archive entry limit exceeded")
        }
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
        let entry_type = e.header().entry_type();
        if entry_type.is_symlink()
            || entry_type.is_hard_link()
            || entry_type.is_block_special()
            || entry_type.is_character_special()
            || entry_type.is_fifo()
        {
            bail!("links are not allowed in archives")
        }
        if entry_type.is_file() {
            files = files
                .checked_add(1)
                .ok_or_else(|| anyhow::anyhow!("archive file count overflow"))?;
            if files > limits.max_workspace_files {
                bail!("archive file limit exceeded")
            }
            expanded_bytes = expanded_bytes
                .checked_add(e.size())
                .ok_or_else(|| anyhow::anyhow!("archive expanded size overflow"))?;
            if expanded_bytes > limits.max_workspace_bytes {
                bail!("archive expanded size limit exceeded")
            }
        }
        e.unpack(&out)?;
    }
    Ok(())
}

fn ensure_budget(cancellation: &CancellationToken, deadline: Instant) -> Result<()> {
    if cancellation.is_cancelled() {
        bail!("workspace preparation cancelled")
    }
    if Instant::now() >= deadline {
        bail!("workspace preparation timed out")
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    use tokio::time::Instant;
    use tokio_util::sync::CancellationToken;

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

        let cancellation = CancellationToken::new();
        copy_tree(
            source.path(),
            destination.path(),
            &RunnerConfig::default_local().limits,
            &cancellation,
            Instant::now() + std::time::Duration::from_secs(10),
        )
        .unwrap();

        assert!(destination.path().join("lib/main.dart").is_file());
        assert!(!destination.path().join("node_modules").exists());
        assert!(!destination.path().join("packages/app/.dart_tool").exists());
    }

    #[test]
    fn copy_tree_enforces_workspace_limits() {
        let source = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();
        fs::write(source.path().join("large.txt"), "0123456789").unwrap();
        let mut config = RunnerConfig::default_local();
        config.limits.max_workspace_bytes = 4;
        let cancellation = CancellationToken::new();

        let error = copy_tree(
            source.path(),
            destination.path(),
            &config.limits,
            &cancellation,
            Instant::now() + std::time::Duration::from_secs(10),
        )
        .unwrap_err();

        assert!(error.to_string().contains("workspace byte limit"));
    }

    #[tokio::test]
    async fn preparation_honors_cancellation_before_copy() {
        let source = tempfile::tempdir().unwrap();
        fs::write(source.path().join("file.txt"), "content").unwrap();
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        let spec = WorkspaceSpec::Local {
            path: source.path().display().to_string(),
        };

        let result = prepare(
            &spec,
            &RunnerConfig::default_local(),
            &cancellation,
            Instant::now() + std::time::Duration::from_secs(10),
        )
        .await;

        let error = match result {
            Ok(_) => panic!("cancelled preparation must fail"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("cancelled"));
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
        let cancellation = CancellationToken::new();
        let prepared = prepare(
            &spec,
            &RunnerConfig::default_local(),
            &cancellation,
            Instant::now() + std::time::Duration::from_secs(30),
        )
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

    #[tokio::test]
    async fn publish_if_changed_accepts_clean_workspace() {
        let repo = tempfile::tempdir().unwrap();
        git(repo.path(), &["init", "-b", "task/change"]);
        git(repo.path(), &["config", "user.name", "Test"]);
        git(repo.path(), &["config", "user.email", "test@example.com"]);
        fs::write(repo.path().join("README.md"), "clean\n").unwrap();
        git(repo.path(), &["add", "README.md"]);
        git(repo.path(), &["commit", "-m", "initial"]);
        let spec = git_workspace(String::new(), None, None, GitPublishMode::IfChanged);

        publish_git(
            &spec,
            repo.path(),
            &CancellationToken::new(),
            Instant::now() + std::time::Duration::from_secs(10),
        )
        .await
        .unwrap();
        assert_eq!(git(repo.path(), &["rev-list", "--count", "HEAD"]), "1");
    }

    #[tokio::test]
    async fn required_publish_rejects_clean_workspace() {
        let repo = tempfile::tempdir().unwrap();
        git(repo.path(), &["init", "-b", "task/change"]);
        let spec = git_workspace(String::new(), None, None, GitPublishMode::Required);

        let error = publish_git(
            &spec,
            repo.path(),
            &CancellationToken::new(),
            Instant::now() + std::time::Duration::from_secs(10),
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("required changes"));
    }

    #[test]
    fn redacts_git_token_from_git_diagnostics() {
        let message = "fatal: https://bot:super-secret@example.test/repo.git";
        let redacted = redact_git_token(message, "super-secret");

        assert!(!redacted.contains("super-secret"));
        assert!(redacted.contains("<redacted-git-token>"));
    }
}
