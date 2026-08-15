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
pub mod http;
pub mod mock;
use crate::job::{JobResult, JobSpec, LogChunk};
use crate::{config::RunnerConfig, executor::Executor};
use anyhow::Result;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;
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
    async fn log_chunk(&self, lease_id: &str, job_id: &str, chunk: &LogChunk) -> Result<()>;
}
pub async fn run_daemon(config: RunnerConfig) -> Result<()> {
    let plane_client: Arc<dyn ControlPlane> = Arc::new(http::HttpControlPlane::new(&config)?);
    let concurrency = config.concurrency.unwrap_or(1).max(1);
    let permits = Arc::new(Semaphore::new(concurrency));
    let mut jobs = JoinSet::new();

    loop {
        let permit = tokio::select! {
            _ = tokio::signal::ctrl_c() => break,
            permit = permits.clone().acquire_owned() => permit.expect("runner semaphore is never closed"),
        };

        let lease = tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                drop(permit);
                break;
            }
            lease = plane_client.lease() => lease,
        };
        let job = match lease {
            Ok(Some(job)) => LeasedJob {
                lease_id: job.lease_id,
                spec: job.spec,
            },
            Ok(None) => {
                drop(permit);
                if wait_or_shutdown(Duration::from_secs(1)).await {
                    break;
                }
                continue;
            }
            Err(error) => {
                drop(permit);
                warn!(%error, "job lease failed; retrying");
                if wait_or_shutdown(Duration::from_secs(3)).await {
                    break;
                }
                continue;
            }
        };

        let client = plane_client.clone();
        let job_config = config.clone();
        jobs.spawn(async move {
            if let Err(error) = run_leased_job(client, job_config, job).await {
                warn!(%error, "leased job worker failed");
            }
            drop(permit);
        });
    }

    while let Some(result) = jobs.join_next().await {
        if let Err(error) = result {
            warn!(%error, "leased job task panicked or was cancelled");
        }
    }
    Ok(())
}

async fn run_leased_job(
    plane: Arc<dyn ControlPlane>,
    config: RunnerConfig,
    job: LeasedJob,
) -> Result<()> {
    let (log_sender, mut log_receiver) = tokio::sync::mpsc::channel(LOG_QUEUE_CAPACITY);
    let log_plane = plane.clone();
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
        match plane.complete(&job.lease_id, &result).await {
            Ok(()) => return Ok(()),
            Err(error) => {
                warn!(job_id = %job.spec.id, %error, "job completion failed; retrying");
                tokio::time::sleep(Duration::from_secs(3)).await;
            }
        }
    }
}

async fn wait_or_shutdown(duration: Duration) -> bool {
    tokio::select! {
        _ = tokio::signal::ctrl_c() => true,
        _ = tokio::time::sleep(duration) => false,
    }
}
