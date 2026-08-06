pub mod http;
pub mod mock;
use crate::job::{JobResult, JobSpec};
use crate::{config::RunnerConfig, executor::Executor};
use anyhow::Result;

#[derive(Debug, Clone)]
pub struct LeasedJob {
    pub lease_id: String,
    pub spec: JobSpec,
}
#[async_trait::async_trait]
pub trait ControlPlane: Send + Sync {
    async fn lease(&self) -> Result<Option<LeasedJob>>;
    async fn complete(&self, lease_id: &str, result: &JobResult) -> Result<()>;
}
pub async fn run_daemon(config: RunnerConfig) -> Result<()> {
    let plane = http::HttpControlPlane::new(&config)?;
    let plane: &dyn ControlPlane = &plane;
    loop {
        tokio::select! { _=tokio::signal::ctrl_c()=>break, lease=plane.lease()=>{ if let Some(job)=lease? { let ex=crate::executor::docker::DockerExecutor::new(config.clone())?; let result=ex.run(job.spec,None).await?; plane.complete(&job.lease_id,&result).await?; } else { tokio::time::sleep(std::time::Duration::from_secs(1)).await; } } }
    }
    Ok(())
}
