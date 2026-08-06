use anyhow::Result;
use std::{
    fs,
    path::{Path, PathBuf},
};
use walkdir::WalkDir;
/// Persist selected files outside the temporary workspace. An empty pattern list
/// deliberately performs no filesystem writes and returns no export directory.
pub fn export(
    root: &Path,
    patterns: &[String],
    destination: &Path,
    max: u64,
) -> Result<Option<Vec<String>>> {
    if patterns.is_empty() {
        return Ok(None);
    }
    let mut out = Vec::new();
    let mut size = 0;
    for entry in WalkDir::new(root).follow_links(false) {
        let entry = entry?;
        if !entry.file_type().is_file() {
            continue;
        }
        let rel = entry
            .path()
            .strip_prefix(root)?
            .to_string_lossy()
            .replace('\\', "/");
        if !patterns.iter().any(|p| glob_match(p, &rel)) {
            continue;
        }
        let bytes = fs::metadata(entry.path())?.len();
        if size + bytes > max {
            break;
        }
        let target = destination.join(&rel);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(entry.path(), &target)?;
        size += bytes;
        out.push(rel);
    }
    Ok(Some(out))
}

pub fn destination(base: &Path, job_id: &str, attempt: u32) -> PathBuf {
    base.join("artifacts")
        .join(safe_component(job_id))
        .join(attempt.to_string())
}

fn safe_component(value: &str) -> String {
    let component: String = value
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect();
    if component.is_empty() {
        "job".into()
    } else {
        component
    }
}
fn glob_match(p: &str, s: &str) -> bool {
    fn matches(pattern: &[u8], value: &[u8]) -> bool {
        match pattern.split_first() {
            None => value.is_empty(),
            Some((b'*', rest)) => {
                matches(rest, value)
                    || value
                        .first()
                        .map(|_| matches(pattern, &value[1..]))
                        .unwrap_or(false)
            }
            Some((head, rest)) => value
                .first()
                .filter(|byte| *byte == head)
                .map(|_| matches(rest, &value[1..]))
                .unwrap_or(false),
        }
    }
    matches(p.as_bytes(), s.as_bytes())
}
