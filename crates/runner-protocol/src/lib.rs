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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution: Option<ExecutionRequirements>,
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
pub struct JobSpecV1beta1 {
    pub api_version: String,
    pub id: String,
    #[serde(default)]
    pub attempt: u32,
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
    #[serde(default)]
    pub writable_rootfs: bool,
    #[serde(default)]
    pub workflow: Option<WorkflowSpec>,
    pub execution: ExecutionRequirements,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct ExecutionRequirements {
    pub isolation: IsolationLevel,
    #[serde(default)]
    pub capabilities: Vec<String>,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum IsolationLevel {
    #[default]
    Container,
    Sandboxed,
    Dedicated,
}

impl From<JobSpecV1beta1> for JobSpec {
    fn from(spec: JobSpecV1beta1) -> Self {
        Self {
            api_version: spec.api_version,
            id: spec.id,
            attempt: spec.attempt,
            // The adapter keeps the Community executor identity for the
            // existing runtime while execution requirements remain explicit.
            executor: "docker".into(),
            execution: Some(spec.execution),
            image: spec.image,
            command: spec.command,
            working_directory: spec.working_directory,
            workspace: spec.workspace,
            environment_from_runner: spec.environment_from_runner,
            secrets: spec.secrets,
            resources: spec.resources,
            network: spec.network,
            artifacts: spec.artifacts,
            writable_rootfs: spec.writable_rootfs,
            workflow: spec.workflow,
        }
    }
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
        #[serde(default)]
        base_sha: Option<String>,
        #[serde(default)]
        head_sha: Option<String>,
        username: String,
        token: String,
        commit_message: String,
        #[serde(default)]
        publish_mode: GitPublishMode,
    },
}

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GitPublishMode {
    Disabled,
    #[default]
    IfChanged,
    Required,
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

pub fn validate_feedback_file(value: &str) -> Result<std::path::PathBuf> {
    let relative = value
        .strip_prefix("/workspace/")
        .or_else(|| (!value.starts_with('/')).then_some(value))
        .ok_or_else(|| anyhow::anyhow!("feedback_file must be inside /workspace"))?;
    if relative.is_empty()
        || relative.contains('\\')
        || relative.contains(':')
        || relative == "."
        || relative.starts_with("./")
        || relative.contains("/./")
        || relative.ends_with("/.")
    {
        bail!("invalid feedback_file path")
    }
    let path = Path::new(relative);
    if path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                std::path::Component::CurDir
                    | std::path::Component::ParentDir
                    | std::path::Component::RootDir
                    | std::path::Component::Prefix(_)
            )
        })
    {
        bail!("invalid feedback_file path")
    }
    Ok(path.to_path_buf())
}
#[derive(Clone, Deserialize, Serialize)]
pub struct SecretSpec {
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub value: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret_ref: Option<String>,
}
impl fmt::Debug for SecretSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SecretSpec")
            .field("name", &self.name)
            .field("value", &"[REDACTED]")
            .field(
                "secret_ref",
                &self.secret_ref.as_ref().map(|_| "[REDACTED]"),
            )
            .finish()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FailureKind {
    Validation,
    Policy,
    Secret,
    Execution,
    Cancellation,
    Timeout,
    Infrastructure,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FailureInfo {
    pub kind: FailureKind,
    pub code: String,
    pub message: String,
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
    pub failure: Option<FailureInfo>,
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
        Self::failed_from_failure(
            spec,
            FailureInfo {
                kind: FailureKind::Execution,
                code: "executor_error".into(),
                message: error.to_string(),
            },
        )
    }

    pub fn failed_from_failure(spec: &JobSpec, failure: FailureInfo) -> Self {
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
            stderr: failure.message.clone(),
            error_summary: Some(failure.message.clone()),
            failure: Some(failure),
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
    use super::{
        ExecutionRequirements, FailureInfo, FailureKind, IsolationLevel, JobSpec, JobSpecV1beta1,
        SecretSpec, error_summary, validate_feedback_file,
    };

    #[test]
    fn v1alpha1_job_spec_fixture_remains_readable_and_valid() {
        let value: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/job-spec-v1alpha1.json")).unwrap();
        let spec: JobSpec = serde_json::from_value(value.clone()).unwrap();

        spec.validate(false).unwrap();
        assert_eq!(spec.api_version, "omniroute.dev/v1alpha1");
        assert_eq!(spec.id, "fixture-job");
        assert_eq!(serde_json::to_value(spec).unwrap(), value);
    }

    #[test]
    fn v1alpha1_job_result_fixture_preserves_required_shape() {
        let value: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/job-result-v1alpha1.json"))
                .unwrap();

        assert_eq!(value["status"], "completed");
        assert_eq!(value["exit_code"], 0);
        assert_eq!(value["sandbox"]["executor"], "docker");
        assert!(value.get("error_summary").is_none());
        assert!(value.get("failed_phase").is_none());
        assert!(value.get("artifact_dir").is_none());
    }

    #[test]
    fn feedback_file_validation_accepts_workspace_relative_path() {
        assert_eq!(
            validate_feedback_file("/workspace/.runner/feedback.md").unwrap(),
            std::path::PathBuf::from(".runner/feedback.md")
        );
        assert_eq!(
            validate_feedback_file(".runner/feedback.md").unwrap(),
            std::path::PathBuf::from(".runner/feedback.md")
        );
    }

    #[test]
    fn feedback_file_validation_rejects_escape_and_platform_prefixes() {
        for path in [
            "../outside.md",
            "/workspace/../outside.md",
            "/tmp/outside.md",
            "C:\\outside.md",
            "\\\\server\\share\\outside.md",
            ".runner/./feedback.md",
        ] {
            assert!(
                validate_feedback_file(path).is_err(),
                "path should be rejected: {path}"
            );
        }
    }

    #[test]
    fn v1beta1_fixture_adapts_to_canonical_job_spec() {
        let document = include_str!("../tests/fixtures/job-spec-v1beta1.json");
        let value: serde_json::Value = serde_json::from_str(document).unwrap();
        let beta: JobSpecV1beta1 = serde_json::from_value(value).unwrap();
        let spec: JobSpec = beta.into();

        spec.validate(false).unwrap();
        assert_eq!(spec.api_version, "omniroute.dev/v1beta1");
        assert_eq!(spec.executor, "docker");
        assert_eq!(
            spec.execution,
            Some(ExecutionRequirements {
                isolation: IsolationLevel::Container,
                capabilities: vec!["artifacts".into(), "streaming_logs".into()]
            })
        );
    }

    #[test]
    fn loader_adapts_v1beta1_and_rejects_unknown_versions() {
        let spec =
            JobSpec::from_json(include_str!("../tests/fixtures/job-spec-v1beta1.json")).unwrap();
        assert_eq!(spec.id, "fixture-beta-job");

        let error = JobSpec::from_json(r#"{"api_version":"omniroute.dev/v9"}"#).unwrap_err();
        assert!(error.to_string().contains("unsupported api_version"));
    }

    #[test]
    fn v1beta1_requires_execution_requirements() {
        let mut spec = JobSpec {
            api_version: "omniroute.dev/v1beta1".into(),
            id: "missing-execution".into(),
            attempt: 1,
            executor: "docker".into(),
            execution: None,
            image: "example/runner:latest".into(),
            command: vec!["true".into()],
            working_directory: "/workspace".into(),
            workspace: super::WorkspaceSpec::Local { path: ".".into() },
            environment_from_runner: Vec::new(),
            secrets: Vec::new(),
            resources: super::Resources {
                cpu: 1.0,
                memory_mb: 256,
                pids: 64,
                timeout_seconds: 60,
            },
            network: super::NetworkSpec {
                mode: "none".into(),
            },
            artifacts: Vec::new(),
            writable_rootfs: false,
            workflow: None,
        };

        assert!(spec.validate(false).is_err());
        spec.execution = Some(ExecutionRequirements {
            isolation: IsolationLevel::Sandboxed,
            capabilities: vec!["sandboxed".into()],
        });
        assert!(spec.validate(false).is_ok());
    }

    #[test]
    fn secret_ref_is_serialized_without_secret_value() {
        let spec = SecretSpec {
            name: "MODEL_KEY".into(),
            value: String::new(),
            secret_ref: Some("vault://team/model-key".into()),
        };
        let json = serde_json::to_value(&spec).unwrap();

        assert_eq!(json["secret_ref"], "vault://team/model-key");
        assert!(json.get("value").is_none());
        assert!(!format!("{spec:?}").contains("vault://team/model-key"));
    }

    #[test]
    fn failed_result_contains_typed_failure() {
        let spec =
            JobSpec::from_json(include_str!("../tests/fixtures/job-spec-v1alpha1.json")).unwrap();
        let result = super::JobResult::failed_from_failure(
            &spec,
            FailureInfo {
                kind: FailureKind::Secret,
                code: "secret_ref_unresolvable".into(),
                message: "secret_ref is not resolvable by the Community executor".into(),
            },
        );
        let json = serde_json::to_value(result).unwrap();

        assert_eq!(json["failure"]["kind"], "secret");
        assert_eq!(json["failure"]["code"], "secret_ref_unresolvable");
    }

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
        let document =
            fs::read_to_string(p).with_context(|| format!("read job {}", p.display()))?;
        Self::from_json(&document)
    }

    pub fn from_json(document: &str) -> Result<Self> {
        let value: serde_json::Value = serde_json::from_str(document).context("parse job JSON")?;
        match value.get("api_version").and_then(serde_json::Value::as_str) {
            Some("omniroute.dev/v1alpha1") => {
                serde_json::from_value(value).context("parse v1alpha1 job JSON")
            }
            Some("omniroute.dev/v1beta1") => {
                let spec: JobSpecV1beta1 =
                    serde_json::from_value(value).context("parse v1beta1 job JSON")?;
                Ok(spec.into())
            }
            Some(version) => bail!("unsupported api_version: {version}"),
            None => bail!("missing api_version"),
        }
    }
    pub fn validate(&self, daemon: bool) -> Result<()> {
        if !["omniroute.dev/v1alpha1", "omniroute.dev/v1beta1"].contains(&self.api_version.as_str())
        {
            bail!("unsupported api_version")
        }
        if self.executor != "docker" {
            bail!("unsupported executor")
        }
        if self.api_version == "omniroute.dev/v1beta1" && self.execution.is_none() {
            bail!("v1beta1 execution requirements are required")
        }
        if let Some(execution) = &self.execution {
            if execution.capabilities.len() > 32 {
                bail!("too many execution capabilities")
            }
            let mut capabilities = std::collections::HashSet::new();
            for capability in &execution.capabilities {
                if capability.is_empty()
                    || capability.len() > 64
                    || !capability
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
                    || !capabilities.insert(capability)
                {
                    bail!("invalid execution capability")
                }
            }
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
            validate_feedback_file(&workflow.feedback_file)?;
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
            if secret.secret_ref.is_some() && !secret.value.is_empty() {
                bail!("secret must use either value or secret_ref")
            }
            if let Some(secret_ref) = &secret.secret_ref
                && (secret_ref.is_empty() || secret_ref.len() > 512)
            {
                bail!("invalid secret_ref")
            }
            if secret.value.len() > 64 * 1024 {
                bail!("secret is too large")
            }
        }
        Ok(())
    }
}
