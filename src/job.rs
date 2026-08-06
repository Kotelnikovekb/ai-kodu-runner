use anyhow::{Context, Result, bail};
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
    Local { path: String },
    ArchiveUrl { url: String },
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
    pub artifacts: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifact_dir: Option<String>,
    pub sandbox: SandboxResult,
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
