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
use anyhow::{Context, Result, bail};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::{fmt, fs, path::Path};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct JobSpec {
    pub api_version: String,
    pub id: String,
    #[serde(default)]
    pub attempt: u32,
    pub executor: String,
    pub image: String,
    #[serde(default)]
    pub command: Vec<String>,
    pub working_directory: String,
    pub workspace: WorkspaceSpec,
    #[serde(default)]
    pub environment_from_runner: Vec<String>,
    #[serde(default)]
    pub secrets: Vec<SecretSpec>,
    pub resources: Resources,
    pub network: NetworkSpec,
    #[serde(default)]
    pub artifacts: Vec<String>,
    /// Explicit compatibility escape hatch for images that must write outside
    /// /workspace, for example Flutter SDK cache or an agent log directory.
    #[serde(default)]
    pub writable_rootfs: bool,
    #[serde(default)]
    pub workflow: Option<WorkflowSpec>,
}
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WorkspaceSpec {
    Local {
        path: String,
    },
    ArchiveUrl {
        url: String,
    },
    Git {
        clone_url: String,
        base_branch: String,
        branch: String,
        username: String,
        token: String,
        commit_message: String,
    },
}
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Resources {
    pub cpu: f64,
    pub memory_mb: i64,
    pub pids: i64,
    pub timeout_seconds: u64,
}
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct NetworkSpec {
    pub mode: String,
}
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct WorkflowSpec {
    #[serde(default)]
    pub setup: Vec<CommandSpec>,
    #[serde(default)]
    pub services: Vec<ServiceSpec>,
    #[serde(default)]
    pub initialize: Option<CommandSpec>,
    pub agent: CommandSpec,
    #[serde(default)]
    pub verifiers: Vec<VerifierSpec>,
    #[serde(default = "default_max_iterations")]
    pub max_iterations: u32,
    #[serde(default = "default_feedback_file")]
    pub feedback_file: String,
    #[serde(default)]
    pub publish: Option<CommandSpec>,
}
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ServiceSpec {
    pub name: String,
    pub image: String,
    #[serde(default)]
    pub alias: Option<String>,
    #[serde(default)]
    pub command: Vec<String>,
    #[serde(default)]
    pub environment: std::collections::HashMap<String, String>,
    #[serde(default)]
    pub healthcheck: Option<HealthcheckSpec>,
}
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct HealthcheckSpec {
    pub command: Vec<String>,
    #[serde(default = "default_healthcheck_timeout")]
    pub timeout_seconds: u64,
}
fn default_healthcheck_timeout() -> u64 {
    60
}
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CommandSpec {
    pub command: Vec<String>,
    #[serde(default)]
    pub working_directory: Option<String>,
}
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct VerifierSpec {
    pub name: String,
    pub command: Vec<String>,
    #[serde(default)]
    pub working_directory: Option<String>,
    #[serde(default)]
    pub required: bool,
}
fn default_max_iterations() -> u32 {
    3
}
fn default_feedback_file() -> String {
    "/workspace/.runner/feedback.md".into()
}
#[derive(Clone, Deserialize, Serialize)]
pub struct SecretSpec {
    pub name: String,
    pub value: String,
}
impl fmt::Debug for SecretSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SecretSpec")
            .field("name", &self.name)
            .field("value", &"[REDACTED]")
            .finish()
    }
}
#[derive(Debug, Clone, Serialize)]
pub struct JobResult {
    pub job_id: String,
    pub attempt: u32,
    pub status: String,
    pub exit_code: Option<i64>,
    pub started_at: String,
    pub finished_at: String,
    pub duration_ms: u128,
    pub log_truncated: bool,
    pub stdout: String,
    pub stderr: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failed_phase: Option<String>,
    pub artifacts: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifact_dir: Option<String>,
    pub sandbox: SandboxResult,
}

#[derive(Debug, Clone, Serialize)]
pub struct LogChunk {
    pub attempt: u32,
    pub sequence: u64,
    pub stream: String,
    pub phase: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub level: Option<String>,
    pub message: String,
}

pub fn error_summary(status: &str, stdout: &str, stderr: &str) -> Option<String> {
    if status == "completed" {
        return None;
    }
    let source = if stderr.trim().is_empty() {
        stdout
    } else {
        stderr
    };
    let line = source.lines().rev().find(|line| !line.trim().is_empty())?;
    let summary = line.trim();
    Some(summary.chars().take(500).collect())
}

impl JobResult {
    pub fn failed_from_error(spec: &JobSpec, error: impl std::fmt::Display) -> Self {
        let now = Utc::now().to_rfc3339();
        Self {
            job_id: spec.id.clone(),
            attempt: spec.attempt,
            status: "failed".into(),
            exit_code: None,
            started_at: now.clone(),
            finished_at: now,
            duration_ms: 0,
            log_truncated: false,
            stdout: String::new(),
            stderr: error.to_string(),
            error_summary: Some(error.to_string()),
            failed_phase: None,
            artifacts: Vec::new(),
            artifact_dir: None,
            sandbox: SandboxResult {
                executor: spec.executor.clone(),
                container_id: String::new(),
                image_id: None,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::error_summary;

    #[test]
    fn error_summary_prefers_stderr_and_truncates() {
        assert_eq!(
            error_summary("failed", "agent output", "first\nUnexpected server error\n"),
            Some("Unexpected server error".into())
        );
        assert_eq!(error_summary("completed", "", ""), None);
    }
}
#[derive(Debug, Clone, Serialize)]
pub struct SandboxResult {
    pub executor: String,
    pub container_id: String,
    pub image_id: Option<String>,
}
impl JobSpec {
    pub fn from_path(p: &Path) -> Result<Self> {
        serde_json::from_str(
            &fs::read_to_string(p).with_context(|| format!("read job {}", p.display()))?,
        )
        .context("parse job JSON")
    }
    pub fn validate(&self, daemon: bool) -> Result<()> {
        if self.api_version != "omniroute.dev/v1alpha1" {
            bail!("unsupported api_version")
        }
        if self.executor != "docker" {
            bail!("unsupported executor")
        }
        if self.workflow.is_none() && self.command.is_empty() {
            bail!("command must not be empty")
        }
        if let Some(workflow) = &self.workflow {
            if workflow
                .initialize
                .as_ref()
                .is_some_and(|command| command.command.is_empty())
            {
                bail!("workflow initialize command must not be empty")
            }
            if workflow.agent.command.is_empty() {
                bail!("workflow agent command must not be empty")
            }
            if workflow.max_iterations == 0 || workflow.max_iterations > 20 {
                bail!("workflow max_iterations must be between 1 and 20")
            }
            if workflow
                .verifiers
                .iter()
                .any(|v| v.name.trim().is_empty() || v.command.is_empty())
            {
                bail!("workflow verifier names and commands must not be empty")
            }
            let mut service_names = std::collections::HashSet::new();
            if workflow.services.len() > 16 {
                bail!("too many workflow services")
            }
            for service in &workflow.services {
                if service.name.trim().is_empty()
                    || !service_names.insert(&service.name)
                    || service.image.trim().is_empty()
                    || service.image.contains(char::is_whitespace)
                    || !service
                        .name
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-' || b == b'.')
                {
                    bail!("invalid workflow service")
                }
                if daemon && !service.image.contains("@sha256:") {
                    bail!("daemon workflow services require image digests")
                }
                if let Some(alias) = &service.alias
                    && (alias.trim().is_empty()
                        || alias.contains(char::is_whitespace)
                        || !alias.bytes().all(|b| {
                            b.is_ascii_alphanumeric() || b == b'_' || b == b'-' || b == b'.'
                        }))
                {
                    bail!("invalid workflow service alias")
                }
                if let Some(healthcheck) = &service.healthcheck
                    && (healthcheck.command.is_empty()
                        || healthcheck.timeout_seconds == 0
                        || healthcheck.timeout_seconds > 3600)
                {
                    bail!("invalid workflow service healthcheck")
                }
            }
        }
        if self.image.trim().is_empty() || self.image.contains(char::is_whitespace) {
            bail!("invalid image reference")
        }
        if daemon && !self.image.contains("@sha256:") {
            bail!("daemon jobs require an image digest")
        }
        if !["bridge", "none"].contains(&self.network.mode.as_str()) {
            bail!("unsupported network mode")
        }
        if self.secrets.len() > 32 {
            bail!("too many secrets")
        }
        for secret in &self.secrets {
            if secret.name.is_empty()
                || secret.name.len() > 128
                || !secret
                    .name
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_')
            {
                bail!("invalid secret name")
            }
            if secret.value.len() > 64 * 1024 {
                bail!("secret is too large")
            }
        }
        Ok(())
    }
}
