pub mod http;
pub mod mock;
use crate::job::{JobResult, JobSpec};
use crate::{config::RunnerConfig, executor::Executor};
use anyhow::Result;
use std::time::Duration;
use tracing::warn;

const LOG_QUEUE_CAPACITY: usize = 256;
const LOG_UPLOAD_ATTEMPTS: usize = 3;
const LOG_UPLOAD_ATTEMPT_TIMEOUT: Duration = Duration::from_secs(10);
const LOG_UPLOAD_RETRY_DELAY: Duration = Duration::from_secs(1);
const LOG_FLUSH_TIMEOUT: Duration = Duration::from_secs(15);

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
    let plane_client = http::HttpControlPlane::new(&config)?;
    let plane: &dyn ControlPlane = &plane_client;
    'daemon: loop {
        let lease = tokio::select! {
            _ = tokio::signal::ctrl_c() => break,
            lease = plane.lease() => lease,
        };
        let job = match lease {
            Ok(Some(job)) => job,
            Ok(None) => {
                if wait_or_shutdown(Duration::from_secs(1)).await {
                    break;
                }
                continue;
            }
            Err(error) => {
                warn!(%error, "job lease failed; retrying");
                if wait_or_shutdown(Duration::from_secs(3)).await {
                    break;
                }
                continue;
            }
        };

        let (log_sender, mut log_receiver) = tokio::sync::mpsc::channel(LOG_QUEUE_CAPACITY);
        let log_plane = plane_client.clone();
        let log_lease = job.lease_id.clone();
        let log_job_id = job.spec.id.clone();
        let mut log_upload = tokio::spawn(async move {
            while let Some(chunk) = log_receiver.recv().await {
                let mut delivered = false;
                for attempt in 1..=LOG_UPLOAD_ATTEMPTS {
                    match tokio::time::timeout(
                        LOG_UPLOAD_ATTEMPT_TIMEOUT,
                        log_plane.log_chunk(&log_lease, &log_job_id, &chunk),
                    )
                    .await
                    {
                        Ok(Ok(())) => {
                            delivered = true;
                            break;
                        }
                        Ok(Err(error)) => {
                            warn!(job_id = %log_job_id, sequence = chunk.sequence, attempt, %error, "job log upload failed");
                        }
                        Err(_) => {
                            warn!(job_id = %log_job_id, sequence = chunk.sequence, attempt, "job log upload timed out");
                        }
                    }
                    if attempt < LOG_UPLOAD_ATTEMPTS {
                        tokio::time::sleep(LOG_UPLOAD_RETRY_DELAY).await;
                    }
                }
                if !delivered {
                    warn!(job_id = %log_job_id, sequence = chunk.sequence, "dropping log chunk after retry budget was exhausted");
                }
            }
        });
        let result = match crate::executor::docker::DockerExecutor::with_log_sender(
            config.clone(),
            Some(log_sender.clone()),
        ) {
            Ok(executor) => match executor.run(job.spec.clone(), None).await {
                Ok(result) => result,
                Err(error) => {
                    warn!(job_id = %job.spec.id, %error, "job execution failed");
                    crate::job::JobResult::failed_from_error(&job.spec, error)
                }
            },
            Err(error) => {
                warn!(job_id = %job.spec.id, %error, "executor initialization failed");
                crate::job::JobResult::failed_from_error(&job.spec, error)
            }
        };
        drop(log_sender);
        if tokio::time::timeout(LOG_FLUSH_TIMEOUT, &mut log_upload)
            .await
            .is_err()
        {
            warn!(job_id = %job.spec.id, "log flush deadline exceeded; completing job without remaining log chunks");
            log_upload.abort();
            let _ = log_upload.await;
        }

        loop {
            let completion = tokio::select! {
                _ = tokio::signal::ctrl_c() => break 'daemon,
                completion = plane.complete(&job.lease_id, &result) => completion,
            };
            match completion {
                Ok(()) => break,
                Err(error) => {
                    warn!(job_id = %job.spec.id, %error, "job completion failed; retrying");
                    if wait_or_shutdown(Duration::from_secs(3)).await {
                        break 'daemon;
                    }
                }
            }
        }
    }
    Ok(())
}

async fn wait_or_shutdown(duration: Duration) -> bool {
    tokio::select! {
        _ = tokio::signal::ctrl_c() => true,
        _ = tokio::time::sleep(duration) => false,
    }
}
