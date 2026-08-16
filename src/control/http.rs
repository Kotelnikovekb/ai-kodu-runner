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
use crate::{
    config::RunnerConfig,
    control::{ControlPlane, HeartbeatDecision, LeasedJob},
    job::{JobResult, JobSpec, LogChunk},
};
use anyhow::{Context, Result};
use reqwest::Client;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use std::time::Duration;

#[derive(Debug)]
pub struct Lease {
    pub lease_id: String,
    pub spec: JobSpec,
}

#[derive(Debug, Deserialize)]
struct RawLease {
    lease_id: String,
    spec: Value,
}

impl<'de> Deserialize<'de> for Lease {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = RawLease::deserialize(deserializer)?;
        let document = serde_json::to_string(&raw.spec).map_err(serde::de::Error::custom)?;
        let spec = JobSpec::from_json(&document).map_err(serde::de::Error::custom)?;
        Ok(Self {
            lease_id: raw.lease_id,
            spec,
        })
    }
}
#[derive(Clone)]
pub struct HttpControlPlane {
    client: Client,
    base: String,
    token: String,
    runner: String,
}
impl HttpControlPlane {
    pub fn new(c: &RunnerConfig) -> Result<Self> {
        Ok(Self {
            client: Client::builder()
                .use_rustls_tls()
                .connect_timeout(Duration::from_secs(5))
                .timeout(Duration::from_secs(30))
                .build()?,
            base: c
                .server_url
                .clone()
                .context("server_url is required for daemon")?,
            token: c
                .resolve_token()?
                .context("runner_token is required for daemon")?,
            runner: c.runner_id(),
        })
    }
    async fn request(&self, b: reqwest::RequestBuilder) -> Result<reqwest::Response> {
        Ok(b.bearer_auth(&self.token)
            .send()
            .await?
            .error_for_status()?)
    }
    pub async fn lease(&self) -> Result<Option<Lease>> {
        let r = self
            .request(
                self.client
                    .post(format!(
                        "{}/v1/runner/lease",
                        self.base.trim_end_matches('/')
                    ))
                    .json(&serde_json::json!({"runner_id":self.runner})),
            )
            .await?;
        if r.status() == reqwest::StatusCode::NO_CONTENT {
            return Ok(None);
        }
        Ok(Some(r.json().await?))
    }
    pub async fn complete(&self, lease: &str, result: &JobResult) -> Result<()> {
        self.request(
            self.client
                .post(format!(
                    "{}/v1/runner/jobs/{}/complete",
                    self.base.trim_end_matches('/'),
                    result.job_id
                ))
                .header("x-lease-id", lease)
                .header("x-job-attempt", result.attempt.to_string())
                .header(
                    "Idempotency-Key",
                    completion_idempotency_key(&result.job_id, result.attempt),
                )
                .json(result),
        )
        .await?;
        Ok(())
    }
    pub async fn heartbeat(&self, lease: &str) -> Result<HeartbeatDecision> {
        let response = self
            .client
            .post(format!(
                "{}/v1/runner/leases/{}/heartbeat",
                self.base.trim_end_matches('/'),
                lease
            ))
            .header("x-lease-id", lease)
            .bearer_auth(&self.token)
            .send()
            .await?;
        if response.status() == reqwest::StatusCode::NO_CONTENT {
            return Ok(HeartbeatDecision::Continue);
        }
        if matches!(
            response.status(),
            reqwest::StatusCode::CONFLICT | reqwest::StatusCode::GONE
        ) {
            return Ok(HeartbeatDecision::LeaseLost);
        }
        Ok(response.error_for_status()?.json().await?)
    }
    pub async fn log_chunk(&self, lease: &str, job_id: &str, chunk: &LogChunk) -> Result<()> {
        self.request(
            self.client
                .post(format!(
                    "{}/v1/runner/jobs/{}/logs",
                    self.base.trim_end_matches('/'),
                    job_id
                ))
                .header("x-lease-id", lease)
                .json(chunk),
        )
        .await?;
        Ok(())
    }
}
#[async_trait::async_trait]
impl ControlPlane for HttpControlPlane {
    async fn lease(&self) -> Result<Option<LeasedJob>> {
        Ok(self.lease().await?.map(|l| LeasedJob {
            lease_id: l.lease_id,
            spec: l.spec,
        }))
    }

    async fn heartbeat(&self, lease_id: &str) -> Result<HeartbeatDecision> {
        self.heartbeat(lease_id).await
    }
    async fn complete(&self, lease_id: &str, result: &JobResult) -> Result<()> {
        self.complete(lease_id, result).await
    }

    async fn log_chunk(&self, lease_id: &str, job_id: &str, chunk: &LogChunk) -> Result<()> {
        self.log_chunk(lease_id, job_id, chunk).await
    }
}

pub fn completion_idempotency_key(job_id: &str, attempt: u32) -> String {
    format!("{job_id}:{attempt}")
}
#[allow(dead_code)]
#[derive(Debug, Serialize)]
pub struct Event<'a> {
    pub state: &'a str,
}

#[cfg(test)]
mod tests {
    use super::{Lease, completion_idempotency_key};

    #[test]
    fn lease_deserialization_uses_version_aware_job_decoder() {
        let fixture =
            include_str!("../../crates/runner-protocol/tests/fixtures/job-spec-v1beta1.json");
        let document = format!(r#"{{"lease_id":"lease-beta","spec":{fixture}}}"#);
        let lease: Lease = serde_json::from_str(&document).unwrap();

        assert_eq!(lease.lease_id, "lease-beta");
        assert_eq!(lease.spec.api_version, "ai-kodu-runner.dev/v1beta1");
        assert_eq!(lease.spec.executor, "docker");
        assert!(lease.spec.execution.is_some());
    }

    #[test]
    fn lease_deserialization_accepts_legacy_v1alpha1() {
        let fixture =
            include_str!("../../crates/runner-protocol/tests/fixtures/job-spec-v1alpha1.json");
        let document = format!(r#"{{"lease_id":"lease-alpha","spec":{fixture}}}"#);
        let lease: Lease = serde_json::from_str(&document).unwrap();

        assert_eq!(lease.lease_id, "lease-alpha");
        assert_eq!(lease.spec.api_version, "ai-kodu-runner.dev/v1alpha1");
        assert_eq!(lease.spec.executor, "docker");
        assert!(lease.spec.execution.is_none());
    }

    #[test]
    fn lease_deserialization_rejects_unknown_job_version() {
        let document = r#"{
            "lease_id": "lease-unknown",
            "spec": {
                "api_version": "ai-kodu-runner.dev/v9",
                "id": "job-unknown"
            }
        }"#;

        let error = serde_json::from_str::<Lease>(document).unwrap_err();
        assert!(error.to_string().contains("unsupported api_version"));
    }

    #[test]
    fn completion_key_is_stable_for_job_attempt() {
        assert_eq!(completion_idempotency_key("job-123", 4), "job-123:4");
        assert_eq!(completion_idempotency_key("job-123", 4), "job-123:4");
        assert_ne!(
            completion_idempotency_key("job-123", 4),
            completion_idempotency_key("job-123", 5)
        );
    }
}
