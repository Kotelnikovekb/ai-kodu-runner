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
use crate::job::{FailureInfo, FailureKind, JobResult, JobSpec, LogChunk};
use crate::{config::RunnerConfig, executor};
use anyhow::Result;
use runner_core::journal::Journal;
use runner_core::policy::{self, ValidationContext};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use tracing::warn;

const LOG_QUEUE_CAPACITY: usize = 256;
const LOG_UPLOAD_ATTEMPTS: usize = 3;
const LOG_UPLOAD_ATTEMPT_TIMEOUT: Duration = Duration::from_secs(10);
const LOG_UPLOAD_RETRY_DELAY: Duration = Duration::from_secs(1);
const LOG_FLUSH_TIMEOUT: Duration = Duration::from_secs(15);
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(10);
const HEARTBEAT_FAILURE_LIMIT: usize = 3;
const COMPLETION_ATTEMPTS: usize = 5;
const COMPLETION_ATTEMPT_TIMEOUT: Duration = Duration::from_secs(10);
const COMPLETION_RETRY_DELAY: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HeartbeatDecision {
    Continue,
    Cancel,
    LeaseLost,
}

#[derive(Debug, Clone)]
pub struct LeasedJob {
    pub lease_id: String,
    pub spec: JobSpec,
}
#[async_trait::async_trait]
pub trait ControlPlane: Send + Sync {
    async fn lease(&self) -> Result<Option<LeasedJob>>;
    async fn heartbeat(&self, lease_id: &str) -> Result<HeartbeatDecision>;
    async fn complete(&self, lease_id: &str, result: &JobResult) -> Result<()>;
    async fn log_chunk(&self, lease_id: &str, job_id: &str, chunk: &LogChunk) -> Result<()>;
}

fn failed_result(
    spec: &JobSpec,
    kind: FailureKind,
    code: &'static str,
    phase: &'static str,
    error: impl std::fmt::Display,
) -> JobResult {
    let mut result = JobResult::failed_from_failure(
        spec,
        FailureInfo {
            kind,
            code: code.into(),
            message: error.to_string(),
        },
    );
    result.failed_phase = Some(phase.into());
    result
}

fn failed_executor_result(spec: &JobSpec, error: &anyhow::Error) -> JobResult {
    if let Some(docker_error) = error.downcast_ref::<executor_docker::DockerFailure>() {
        let mut result = JobResult::failed_from_failure(spec, docker_error.failure_info());
        result.failed_phase = Some(docker_error.phase.into());
        return result;
    }
    failed_result(
        spec,
        FailureKind::Infrastructure,
        "executor_operation_failed",
        "execution",
        error,
    )
}
pub async fn run_daemon(config: RunnerConfig) -> Result<()> {
    let plane_client: Arc<dyn ControlPlane> = Arc::new(http::HttpControlPlane::new(&config)?);
    let journal = Journal::open(&config.work_dir.join("runner.db"))?;
    replay_pending_completions(plane_client.clone(), journal.clone()).await;
    let concurrency = config.concurrency.unwrap_or(1).max(1);
    let permits = Arc::new(Semaphore::new(concurrency));
    let shutdown = CancellationToken::new();
    let signal_shutdown = shutdown.clone();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            signal_shutdown.cancel();
        }
    });
    let mut jobs = JoinSet::new();

    loop {
        let permit = tokio::select! {
            _ = shutdown.cancelled() => break,
            permit = permits.clone().acquire_owned() => permit.expect("runner semaphore is never closed"),
        };

        let lease = tokio::select! {
            _ = shutdown.cancelled() => {
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
                if wait_or_shutdown(&shutdown, Duration::from_secs(1)).await {
                    break;
                }
                continue;
            }
            Err(error) => {
                drop(permit);
                warn!(%error, "job lease failed; retrying");
                if wait_or_shutdown(&shutdown, Duration::from_secs(3)).await {
                    break;
                }
                continue;
            }
        };

        let client = plane_client.clone();
        let job_config = config.clone();
        let job_shutdown = shutdown.child_token();
        jobs.spawn(async move {
            if let Err(error) = run_leased_job(client, job_config, job, job_shutdown).await {
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
    cancellation: CancellationToken,
) -> Result<()> {
    let heartbeat_plane = plane.clone();
    let heartbeat_lease = job.lease_id.clone();
    let heartbeat_cancel = cancellation.clone();
    let heartbeat = tokio::spawn(async move {
        let mut failures = 0usize;
        loop {
            tokio::select! {
                _ = heartbeat_cancel.cancelled() => break HeartbeatDecision::Cancel,
                _ = tokio::time::sleep(HEARTBEAT_INTERVAL) => {
                    match heartbeat_plane.heartbeat(&heartbeat_lease).await {
                        Ok(HeartbeatDecision::Continue) => failures = 0,
                        Ok(decision @ (HeartbeatDecision::Cancel | HeartbeatDecision::LeaseLost)) => {
                            warn!(lease_id = %heartbeat_lease, ?decision, "lease no longer active");
                            heartbeat_cancel.cancel();
                            break decision;
                        }
                        Err(error) => {
                            failures += 1;
                            warn!(lease_id = %heartbeat_lease, failures, %error, "lease heartbeat failed");
                            if failures >= HEARTBEAT_FAILURE_LIMIT {
                                heartbeat_cancel.cancel();
                                break HeartbeatDecision::LeaseLost;
                            }
                        }
                    }
                }
            }
        }
    });
    let (log_sender, mut log_receiver) = tokio::sync::mpsc::channel(LOG_QUEUE_CAPACITY);
    let log_plane = plane.clone();
    let log_lease = job.lease_id.clone();
    let log_job_id = job.spec.id.clone();
    let mut log_upload = tokio::spawn(async move {
        let mut loss = false;
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
                loss = true;
                warn!(job_id = %log_job_id, sequence = chunk.sequence, "dropping log chunk after retry budget was exhausted");
            }
        }
        loss
    });
    let mut result =
        match policy::validate_with_context(&job.spec, &config, ValidationContext::Daemon) {
            Ok(_) => match executor::factory(config.clone(), Some(log_sender.clone())) {
                Ok(factory) => match factory.select_for(&job.spec) {
                    Ok(executor) => match executor
                        .run(job.spec.clone(), Some(cancellation.clone()))
                        .await
                    {
                        Ok(result) => result,
                        Err(error) => {
                            warn!(job_id = %job.spec.id, %error, "job execution failed");
                            if cancellation.is_cancelled() {
                                failed_result(
                                    &job.spec,
                                    FailureKind::Cancellation,
                                    "cancelled",
                                    "execution",
                                    error,
                                )
                            } else {
                                failed_executor_result(&job.spec, &error)
                            }
                        }
                    },
                    Err(error) => {
                        warn!(job_id = %job.spec.id, %error, "executor admission failed");
                        failed_result(
                            &job.spec,
                            FailureKind::Policy,
                            "executor_admission_rejected",
                            "admission",
                            error,
                        )
                    }
                },
                Err(error) => {
                    warn!(job_id = %job.spec.id, %error, "executor initialization failed");
                    failed_result(
                        &job.spec,
                        FailureKind::Infrastructure,
                        "executor_initialization_failed",
                        "initialization",
                        error,
                    )
                }
            },
            Err(error) => {
                warn!(job_id = %job.spec.id, %error, "daemon job admission failed");
                failed_result(
                    &job.spec,
                    FailureKind::Policy,
                    "job_admission_rejected",
                    "admission",
                    error,
                )
            }
        };
    drop(log_sender);
    let log_loss = tokio::time::timeout(LOG_FLUSH_TIMEOUT, &mut log_upload)
        .await
        .map(|result| result.unwrap_or(true))
        .unwrap_or(true);
    if log_loss {
        warn!(job_id = %job.spec.id, "log flush deadline exceeded; completing job without remaining log chunks");
        log_upload.abort();
        let _ = log_upload.await;
    }
    cancellation.cancel();
    let _ = heartbeat.await;
    if log_loss {
        result.log_truncated = true;
    }

    let journal = Journal::open(&config.work_dir.join("runner.db"))?;
    let payload = serde_json::to_string(&result)?;
    let idempotency_key = http::completion_idempotency_key(&result.job_id, result.attempt);
    journal.enqueue_completion(
        &result.job_id,
        result.attempt,
        &job.lease_id,
        &idempotency_key,
        &payload,
    )?;

    for attempt in 1..=COMPLETION_ATTEMPTS {
        journal.record_completion_attempt(&result.job_id, result.attempt)?;
        match tokio::time::timeout(
            COMPLETION_ATTEMPT_TIMEOUT,
            plane.complete(&job.lease_id, &result),
        )
        .await
        {
            Ok(Ok(())) => {
                journal.mark_completion_delivered(&result.job_id, result.attempt)?;
                return Ok(());
            }
            Ok(Err(error)) => {
                warn!(job_id = %job.spec.id, %error, "job completion failed; retrying");
            }
            Err(_) => warn!(job_id = %job.spec.id, "job completion timed out; retrying"),
        }
        if attempt < COMPLETION_ATTEMPTS {
            tokio::time::sleep(COMPLETION_RETRY_DELAY).await;
        }
    }
    anyhow::bail!("job completion retry budget exhausted")
}

async fn replay_pending_completions(plane: Arc<dyn ControlPlane>, journal: Journal) {
    let pending = match journal.pending_completions() {
        Ok(pending) => pending,
        Err(error) => {
            warn!(%error, "pending completion outbox unavailable");
            return;
        }
    };
    for entry in pending {
        let result = match serde_json::from_str::<JobResult>(&entry.payload) {
            Ok(result) => result,
            Err(error) => {
                warn!(job_id = %entry.job_id, attempt = entry.attempt, %error, "invalid pending completion payload");
                continue;
            }
        };
        if entry.idempotency_key != http::completion_idempotency_key(&result.job_id, result.attempt)
        {
            warn!(
                job_id = %entry.job_id,
                attempt = entry.attempt,
                "pending completion idempotency key does not match payload"
            );
            continue;
        }
        if let Err(error) = journal.record_completion_attempt(&entry.job_id, entry.attempt) {
            warn!(job_id = %entry.job_id, attempt = entry.attempt, %error, "cannot record pending completion attempt");
            continue;
        }
        match plane.complete(&entry.lease_id, &result).await {
            Ok(()) => {
                if let Err(error) = journal.mark_completion_delivered(&entry.job_id, entry.attempt)
                {
                    warn!(job_id = %entry.job_id, attempt = entry.attempt, %error, "cannot mark pending completion delivered");
                }
            }
            Err(error) => {
                warn!(job_id = %entry.job_id, attempt = entry.attempt, %error, "pending completion replay failed")
            }
        }
    }
}

async fn wait_or_shutdown(shutdown: &CancellationToken, duration: Duration) -> bool {
    tokio::select! {
        _ = shutdown.cancelled() => true,
        _ = tokio::time::sleep(duration) => false,
    }
}
