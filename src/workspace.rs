use crate::{config::RunnerConfig, job::WorkspaceSpec};
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
            base_branch: _,
            branch,
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
            let branch_output = Command::new("git")
                .args(["checkout", "-B", branch])
                .current_dir(dir.path())
                .output()
                .context("create git branch")?;
            if !branch_output.status.success() {
                let stderr = String::from_utf8_lossy(&branch_output.stderr);
                bail!("git branch creation failed: {}", stderr.trim())
            }
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
        ..
    } = spec
    else {
        return Ok(());
    };
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
}
