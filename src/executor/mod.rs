// Compatibility facade: implementations live in workspace crates.
use crate::config::RunnerConfig;
use anyhow::Result;
use runner_core::executor::{DoctorReport, ExecutorFactory};
use runner_protocol::LogChunk;
use std::sync::Arc;
use tokio::sync::mpsc::Sender;

pub use runner_core::executor::Executor;

pub fn factory(
    config: RunnerConfig,
    log_sender: Option<Sender<LogChunk>>,
) -> Result<ExecutorFactory> {
    let executor = executor_docker::DockerExecutor::with_log_sender(config, log_sender)?;
    let mut factory = ExecutorFactory::default();
    factory.register("docker", Arc::new(executor))?;
    Ok(factory)
}

pub async fn doctor() -> Result<()> {
    let factory = factory(RunnerConfig::default_local(), None)?;
    let executor = factory.get("docker")?;
    let report: DoctorReport = executor.doctor().await?;
    println!("executor: {}", report.executor);
    println!("healthy: {}", report.healthy);
    println!(
        "capabilities: {}",
        report
            .capabilities
            .capabilities
            .into_iter()
            .collect::<Vec<_>>()
            .join(", ")
    );
    for check in report.checks {
        println!("{}: {} ({})", check.name, check.healthy, check.message);
    }
    Ok(())
}
