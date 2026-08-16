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

use crate::executor::{DoctorCheck, DoctorReport, Executor, ExecutorCapabilities};
use anyhow::Result;
use chrono::Utc;
use runner_protocol::{FailureInfo, FailureKind, JobResult, JobSpec, SandboxResult};
use tokio_util::sync::CancellationToken;

#[derive(Debug, Default)]
pub struct MockExecutor;

#[async_trait::async_trait]
impl Executor for MockExecutor {
    fn capabilities(&self) -> ExecutorCapabilities {
        ExecutorCapabilities::new(["artifacts", "cancellation", "streaming_logs"])
    }

    async fn doctor(&self) -> Result<DoctorReport> {
        Ok(DoctorReport {
            executor: "mock".into(),
            healthy: true,
            capabilities: self.capabilities(),
            checks: vec![DoctorCheck {
                name: "mock".into(),
                healthy: true,
                message: "mock executor is ready".into(),
            }],
        })
    }

    async fn run(&self, spec: JobSpec, cancel: Option<CancellationToken>) -> Result<JobResult> {
        let now = Utc::now().to_rfc3339();
        let cancelled = cancel.is_some_and(|token| token.is_cancelled());
        Ok(JobResult {
            job_id: spec.id,
            attempt: spec.attempt,
            status: if cancelled { "cancelled" } else { "completed" }.into(),
            exit_code: (!cancelled).then_some(0),
            started_at: now.clone(),
            finished_at: now,
            duration_ms: 0,
            log_truncated: false,
            stdout: String::new(),
            stderr: String::new(),
            error_summary: None,
            failure: cancelled.then(|| FailureInfo {
                kind: FailureKind::Cancellation,
                code: "cancelled".into(),
                message: "execution cancelled".into(),
            }),
            failed_phase: None,
            artifacts: Vec::new(),
            artifact_dir: None,
            sandbox: SandboxResult {
                executor: "mock".into(),
                container_id: String::new(),
                image_id: None,
            },
        })
    }

    async fn cleanup(&self) -> Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::MockExecutor;
    use crate::executor::Executor;

    #[tokio::test]
    async fn doctor_is_healthy() {
        let report = MockExecutor.doctor().await.unwrap();

        assert!(report.healthy);
        assert!(report.capabilities.supports("cancellation"));
        assert_eq!(report.checks.len(), 1);
    }
}
