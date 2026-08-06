use crate::job::JobSpec;
use anyhow::Result;
use std::path::Path;
#[allow(dead_code)]
pub struct FileControlPlane {
    path: std::path::PathBuf,
}
#[allow(dead_code)]
impl FileControlPlane {
    pub fn new(path: &Path) -> Self {
        Self {
            path: path.to_owned(),
        }
    }
    pub fn read(&self) -> Result<JobSpec> {
        Ok(serde_json::from_slice(&std::fs::read(&self.path)?)?)
    }
}
