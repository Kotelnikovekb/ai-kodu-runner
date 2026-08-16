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
    max_files: usize,
) -> Result<Option<Vec<String>>> {
    if patterns.is_empty() {
        return Ok(None);
    }
    let mut out = Vec::new();
    let mut size = 0;
    for entry in WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| should_visit(entry.path(), root, patterns))
    {
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
        if out.len() >= max_files || size + bytes > max {
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

fn should_visit(path: &Path, root: &Path, patterns: &[String]) -> bool {
    let Ok(rel) = path.strip_prefix(root) else {
        return false;
    };
    if rel.as_os_str().is_empty() || !crate::workspace::is_derived_path(rel) {
        return true;
    }

    // Broad patterns such as "**" must not pull caches into the result. A job
    // can still deliberately export a generated tree with an exact prefix such
    // as "build/**" or ".omniroute/results/**".
    rel.ancestors()
        .filter(|path| !path.as_os_str().is_empty())
        .any(|derived_root| {
            if !crate::workspace::is_derived_path(derived_root) {
                return false;
            }
            let derived_root = derived_root.to_string_lossy().replace('\\', "/");
            patterns.iter().any(|pattern| {
                let pattern = pattern.trim_start_matches("./");
                pattern == derived_root || pattern.starts_with(&format!("{derived_root}/"))
            })
        })
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn broad_export_prunes_generated_directories() {
        let source = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();
        fs::create_dir_all(source.path().join("lib")).unwrap();
        fs::create_dir_all(source.path().join("build/web")).unwrap();
        fs::create_dir_all(source.path().join("node_modules/pkg")).unwrap();
        fs::write(source.path().join("lib/main.dart"), "source\n").unwrap();
        fs::write(source.path().join("build/web/app.js"), "generated\n").unwrap();
        fs::write(
            source.path().join("node_modules/pkg/index.js"),
            "dependency\n",
        )
        .unwrap();

        let files = export(
            source.path(),
            &["**".into()],
            destination.path(),
            u64::MAX,
            usize::MAX,
        )
        .unwrap()
        .unwrap();

        assert_eq!(files, vec!["lib/main.dart"]);
        assert!(!destination.path().join("build").exists());
        assert!(!destination.path().join("node_modules").exists());
    }

    #[test]
    fn exact_prefix_can_export_generated_output() {
        let source = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();
        fs::create_dir_all(source.path().join("build/web")).unwrap();
        fs::write(source.path().join("build/web/app.js"), "generated\n").unwrap();

        let files = export(
            source.path(),
            &["build/**".into()],
            destination.path(),
            u64::MAX,
            usize::MAX,
        )
        .unwrap()
        .unwrap();

        assert_eq!(files, vec!["build/web/app.js"]);
    }
}
