pub mod docker;
use crate::job::{JobResult, JobSpec};
use anyhow::Result;
use async_trait::async_trait;
use tokio_util::sync::CancellationToken;
#[async_trait]
pub trait Executor {
    async fn run(&self, spec: JobSpec, cancel: Option<CancellationToken>) -> Result<JobResult>;
}
