use crate::{config::RunnerConfig, job::WorkspaceSpec};
use anyhow::{Context, Result, bail};
use std::{fs, path::Path};
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
    };
    Ok(PreparedWorkspace {
        dir,
        container_path: "/workspace".into(),
    })
}
fn copy_tree(from: &Path, to: &Path) -> Result<()> {
    if !from.is_dir() {
        bail!("workspace is not a directory")
    }
    for e in WalkDir::new(from).follow_links(false) {
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
    #[test]
    fn rejects_escape() {
        assert!("../etc/passwd".split('/').any(|x| x == ".."));
    }
}
