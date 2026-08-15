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
    control::{ControlPlane, LeasedJob},
    job::{JobResult, JobSpec, LogChunk},
};
use anyhow::{Context, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};
#[derive(Debug, Deserialize)]
pub struct Lease {
    pub lease_id: String,
    pub spec: JobSpec,
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
            client: Client::builder().use_rustls_tls().build()?,
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
                .json(result),
        )
        .await?;
        Ok(())
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
    async fn complete(&self, lease_id: &str, result: &JobResult) -> Result<()> {
        self.complete(lease_id, result).await
    }

    async fn log_chunk(&self, lease_id: &str, job_id: &str, chunk: &LogChunk) -> Result<()> {
        self.log_chunk(lease_id, job_id, chunk).await
    }
}
#[allow(dead_code)]
#[derive(Debug, Serialize)]
pub struct Event<'a> {
    pub state: &'a str,
}
