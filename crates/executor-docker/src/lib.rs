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
use anyhow::{Context, Result, anyhow, bail};
use bollard::container::{
    Config, CreateContainerOptions, ListContainersOptions, LogOutput, LogsOptions,
    NetworkingConfig, RemoveContainerOptions, StartContainerOptions, StopContainerOptions,
};
use bollard::exec::{CreateExecOptions, StartExecOptions, StartExecResults};
use bollard::image::CreateImageOptions;
use bollard::models::{EndpointSettings, HealthConfig, HostConfig};
use bollard::network::{CreateNetworkOptions, ListNetworksOptions};
use bollard::{API_DEFAULT_VERSION, Docker};
use futures_util::StreamExt;
use runner_core::executor::{DoctorCheck, DoctorReport, Executor, ExecutorCapabilities};
use runner_core::{
    artifacts,
    config::{RunnerConfig, platform_from_env, validate_platform},
    journal::Journal,
    policy, state, workspace,
};
use runner_protocol::{
    CommandSpec, FailureInfo, FailureKind, JobResult, JobSpec, LogChunk, SandboxResult,
    ServiceSpec, WorkflowSpec, validate_feedback_file,
};
use std::sync::atomic::{AtomicU64, Ordering};
use std::{
    collections::HashMap,
    fs,
    path::{Component, Path, PathBuf},
    sync::{Arc, Mutex},
    time::Instant,
};

/// Stable, public classification for failures originating in the Docker
/// backend. The human-readable message is diagnostic only; callers should use
/// `code` and `phase` for policy, retry and billing decisions.
#[derive(Debug, thiserror::Error)]
#[error("Docker {code} during {phase}: {message}")]
pub struct DockerFailure {
    pub code: &'static str,
    pub phase: &'static str,
    pub message: String,
    pub retryable: bool,
}

impl DockerFailure {
    fn image_pull(error: impl std::fmt::Display) -> Self {
        Self {
            code: "image_pull_failed",
            phase: "image_pull",
            message: sanitize_docker_error(error),
            retryable: true,
        }
    }

    pub fn failure_info(&self) -> FailureInfo {
        FailureInfo {
            kind: FailureKind::Infrastructure,
            code: self.code.into(),
            message: format!("{} (phase: {})", self.message, self.phase),
        }
    }
}

fn sanitize_docker_error(error: impl std::fmt::Display) -> String {
    let message = error.to_string();
    let mut value = serde_json::Value::String(message);
    redact_diagnostic_value(&mut value);
    value
        .as_str()
        .unwrap_or("Docker operation failed")
        .chars()
        .take(500)
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExecutionTermination {
    Cancelled,
    TimedOut,
}

#[derive(Debug)]
struct ExecResult {
    status: i64,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    truncated: bool,
    termination: Option<ExecutionTermination>,
}

fn append_bounded(buffer: &mut Vec<u8>, data: &[u8], limit: u64) -> bool {
    let remaining = limit.saturating_sub(buffer.len() as u64) as usize;
    let amount = data.len().min(remaining);
    buffer.extend_from_slice(&data[..amount]);
    amount < data.len()
}

fn redact_diagnostic_value(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(object) => {
            for (key, value) in object {
                let normalized = key.to_ascii_lowercase();
                if normalized == "command" {
                    *value = serde_json::json!(["<redacted: command may contain credentials>"]);
                } else if normalized == "secrets" {
                    if let serde_json::Value::Array(secrets) = value {
                        for secret in secrets.iter_mut() {
                            if let Some(value) = secret.get_mut("value") {
                                *value = serde_json::Value::String("<redacted>".into());
                            }
                        }
                    }
                    redact_diagnostic_value(value);
                } else if normalized.contains("token")
                    || normalized.contains("password")
                    || normalized.contains("secret")
                    || normalized.contains("authorization")
                    || normalized.ends_with("apikey")
                    || normalized.ends_with("api_key")
                    || normalized.ends_with("_key")
                {
                    *value = serde_json::Value::String("<redacted>".into());
                } else {
                    redact_diagnostic_value(value);
                }
            }
        }
        serde_json::Value::Array(values) => {
            for value in values {
                redact_diagnostic_value(value);
            }
        }
        _ => {}
    }
}

fn redact_diagnostic_bytes(spec: &JobSpec, bytes: &[u8]) -> Vec<u8> {
    const MAX_DIAGNOSTIC_BYTES: usize = 64 * 1024;
    let mut text = String::from_utf8_lossy(bytes).into_owned();
    let mut secrets: Vec<String> = Vec::new();
    if let runner_protocol::WorkspaceSpec::Git { token, .. } = &spec.workspace {
        secrets.push(token.clone());
    }
    for secret in &spec.secrets {
        secrets.push(secret.value.clone());
    }
    for name in &spec.environment_from_runner {
        if let Ok(value) = std::env::var(name) {
            secrets.push(value);
        }
    }
    for secret in secrets {
        if !secret.is_empty() {
            text = text.replace(&secret, "<redacted>");
        }
    }
    let mut output = text.into_bytes();
    if output.len() > MAX_DIAGNOSTIC_BYTES {
        output.truncate(MAX_DIAGNOSTIC_BYTES);
        output.extend_from_slice(b"\n[diagnostic output truncated]\n");
    }
    output
}

fn write_failure_diagnostics(
    workspace: &Path,
    spec: &JobSpec,
    phase: Option<&str>,
    stdout: &[u8],
    stderr: &[u8],
) -> Result<()> {
    let dir = workspace.join(".runner/diagnostics");
    fs::create_dir_all(&dir)?;
    let mut job = serde_json::to_value(spec)?;
    redact_diagnostic_value(&mut job);
    fs::write(dir.join("job.json"), serde_json::to_vec_pretty(&job)?)?;
    fs::write(
        dir.join("meta.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "job_id": spec.id,
            "attempt": spec.attempt,
            "image": spec.image,
            "network": spec.network.mode,
            "phase": phase,
        }))?,
    )?;
    // Command output and agent context may contain credentials or source code.
    // Keep only bounded, value-redacted diagnostics; never copy prompt/AGENTS/
    // feedback files into exported artifacts.
    fs::write(
        dir.join("stdout.log"),
        redact_diagnostic_bytes(spec, stdout),
    )?;
    fs::write(
        dir.join("stderr.log"),
        redact_diagnostic_bytes(spec, stderr),
    )?;
    Ok(())
}
use tokio::sync::mpsc::{Sender, error::TrySendError};
use tracing::{info, warn};
use uuid::Uuid;

/// Writable, ephemeral runtime directories for images that use a read-only
/// root filesystem. The uid/gid match the standard `opencode` user used by
/// the bundled tool images.
fn runtime_tmpfs() -> HashMap<String, String> {
    HashMap::from([
        ("/tmp".into(), "rw,noexec,nosuid,size=256m".into()),
        (
            "/home/opencode/.opencode".into(),
            "rw,nosuid,nodev,uid=10001,gid=10001,mode=0700,size=64m".into(),
        ),
        (
            "/home/opencode/.config/opencode".into(),
            "rw,nosuid,nodev,uid=10001,gid=10001,mode=0700,size=512m".into(),
        ),
        (
            "/home/opencode/.local/share/opencode".into(),
            "rw,nosuid,nodev,uid=10001,gid=10001,mode=0700,size=1g".into(),
        ),
        (
            "/home/opencode/.local/state".into(),
            "rw,nosuid,nodev,uid=10001,gid=10001,mode=0700,size=32m".into(),
        ),
        (
            "/home/opencode/.cache/npm".into(),
            "rw,nosuid,nodev,uid=10001,gid=10001,mode=0700,size=1g".into(),
        ),
        (
            "/home/opencode/.cache/opencode".into(),
            "rw,nosuid,nodev,uid=10001,gid=10001,mode=0700,size=128m".into(),
        ),
        (
            "/home/opencode/.pub-cache".into(),
            "rw,nosuid,nodev,uid=10001,gid=10001,mode=0700,size=1g".into(),
        ),
    ])
}

fn command_with_opencode_retry(command: &[String]) -> Vec<String> {
    if command.len() != 3
        || command[0] != "bash"
        || command[1] != "-lc"
        || !command[2].contains("opencode run ")
        || !command[2].contains(".ai-kodu-runner/results/opencode.json")
    {
        return command.to_vec();
    }

    let script = command[2].replacen(
        "opencode run ",
        "opencode run --print-logs --log-level INFO ",
        1,
    );
    let wrapped = format!(
        r#"set +e
(
{script}
)
runner_rc=$?
if [ "$runner_rc" -ne 0 ] && grep -Fq 'Unexpected server error' .ai-kodu-runner/results/opencode.json 2>/dev/null; then
    echo 'OpenCode reported an unexpected error; retrying once in 2 seconds' >&2
    sleep 2
    (
{script}
    )
    runner_rc=$?
fi
exit "$runner_rc""#
    );

    vec![command[0].clone(), command[1].clone(), wrapped]
}

fn headless_opencode_environment(mut env: Vec<String>, command: &[String]) -> Vec<String> {
    if !command.iter().any(|arg| arg.contains("opencode run ")) {
        return env;
    }

    // Session titles are irrelevant for headless jobs. Disabling the hidden
    // title agent also avoids a second concurrent request through custom
    // OpenAI-compatible providers.
    env.push(r#"OPENCODE_CONFIG_CONTENT={"agent":{"title":{"disable":true}}}"#.into());
    env.push("OPENCODE_DISABLE_MODELS_FETCH=true".into());
    env
}

fn safe_feedback_path(root: &Path, feedback_file: &str) -> Result<PathBuf> {
    let relative = validate_feedback_file(feedback_file)?;
    let root = root
        .canonicalize()
        .context("canonicalize feedback workspace")?;
    let mut current = root.clone();
    if let Some(parent) = relative.parent() {
        for component in parent
            .components()
            .filter(|component| !matches!(component, Component::CurDir))
        {
            current.push(component.as_os_str());
            match fs::symlink_metadata(&current) {
                Ok(metadata) if metadata.file_type().is_symlink() => {
                    bail!("feedback_file parent must not be a symlink")
                }
                Ok(metadata) if !metadata.is_dir() => {
                    bail!("feedback_file parent is not a directory")
                }
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    fs::create_dir(&current)?;
                }
                Err(error) => return Err(error.into()),
            }
        }
    }
    let target = root.join(&relative);
    if let Ok(metadata) = fs::symlink_metadata(&target)
        && metadata.file_type().is_symlink()
    {
        bail!("feedback_file must not be a symlink")
    }
    let parent = target
        .parent()
        .context("feedback_file has no parent directory")?
        .canonicalize()?;
    if !parent.starts_with(&root) {
        bail!("feedback_file escapes workspace")
    }
    Ok(target)
}

pub struct DockerExecutor {
    config: RunnerConfig,
    docker: Docker,
    log_sender: Option<Sender<LogChunk>>,
    log_sequence: AtomicU64,
    dropped_log_chunks: AtomicU64,
    log_context: Arc<Mutex<Option<(String, u32, String)>>>,
}
impl DockerExecutor {
    pub fn new(config: RunnerConfig) -> Result<Self> {
        Self::with_log_sender(config, None)
    }
    pub fn with_log_sender(
        config: RunnerConfig,
        log_sender: Option<Sender<LogChunk>>,
    ) -> Result<Self> {
        validate_platform(&config.docker.platform)?;
        let docker =
            Docker::connect_with_local_defaults().context("connect to Docker Engine/Desktop")?;
        Ok(Self {
            config,
            docker,
            log_sender,
            log_sequence: AtomicU64::new(0),
            dropped_log_chunks: AtomicU64::new(0),
            log_context: Arc::new(Mutex::new(None)),
        })
    }
    fn begin_log_stream(&self, spec: &JobSpec) {
        self.log_sequence.store(0, Ordering::Relaxed);
        self.dropped_log_chunks.store(0, Ordering::Relaxed);
        if let Ok(mut context) = self.log_context.lock() {
            *context = Some((spec.id.clone(), spec.attempt, "running".into()));
        }
    }
    fn set_log_phase(&self, phase: &str) {
        if let Ok(mut context) = self.log_context.lock()
            && let Some((_, _, current_phase)) = context.as_mut()
        {
            *current_phase = phase.into();
        }
    }
    fn emit_log(&self, stream: &str, bytes: &[u8]) {
        let Some(sender) = &self.log_sender else {
            return;
        };
        let Ok(context) = self.log_context.lock() else {
            return;
        };
        let Some((_, attempt, phase)) = context.as_ref() else {
            return;
        };
        for chunk in bytes.chunks(64 * 1024) {
            let sequence = self.log_sequence.fetch_add(1, Ordering::Relaxed) + 1;
            let chunk = LogChunk {
                attempt: *attempt,
                sequence,
                stream: stream.to_owned(),
                phase: phase.clone(),
                level: Some(if stream == "stderr" { "warn" } else { "info" }.into()),
                message: String::from_utf8_lossy(chunk).into_owned(),
            };
            match sender.try_send(chunk) {
                Ok(()) | Err(TrySendError::Closed(_)) => {}
                Err(TrySendError::Full(_)) => {
                    let dropped = self.dropped_log_chunks.fetch_add(1, Ordering::Relaxed) + 1;
                    if dropped == 1 || dropped.is_power_of_two() {
                        warn!(dropped, "job log queue is full; dropping chunks");
                    }
                }
            }
        }
    }
    async fn connect() -> Result<Docker> {
        Docker::connect_with_local_defaults().context("connect to Docker Engine/Desktop")
    }
    pub async fn doctor() -> Result<()> {
        let d = Self::connect().await?;
        d.ping().await.context("Docker ping")?;
        let v = d.version().await?;
        let test = format!("ai-kodu-runner-doctor-{}", Uuid::new_v4());
        let id = d
            .create_container(
                Some(CreateContainerOptions::<String> {
                    name: test.clone(),
                    platform: Some(platform_from_env()),
                }),
                Config::<String> {
                    image: Some("alpine:3.20".into()),
                    cmd: Some(vec!["true".into()]),
                    ..Default::default()
                },
            )
            .await?
            .id;
        d.remove_container(
            &id,
            Some(RemoveContainerOptions {
                force: true,
                ..Default::default()
            }),
        )
        .await?;
        println!(
            "host_os: {}\narchitecture: {}\ndocker_api: {}\nexecutor: docker\ncapabilities: network, artifacts, streaming_logs, cancellation\ntest_container: ok",
            std::env::consts::OS,
            std::env::consts::ARCH,
            v.api_version
                .map(|x| format!("{x:?}"))
                .unwrap_or_else(|| format!("{API_DEFAULT_VERSION:?}"))
        );
        Ok(())
    }
    async fn pull(
        &self,
        image: &str,
        cancellation: &tokio_util::sync::CancellationToken,
        deadline: tokio::time::Instant,
    ) -> Result<Option<ExecutionTermination>> {
        if cancellation.is_cancelled() {
            return Ok(Some(ExecutionTermination::Cancelled));
        }
        if tokio::time::Instant::now() >= deadline {
            return Ok(Some(ExecutionTermination::TimedOut));
        }
        if self.config.docker.pull_policy.as_deref() == Some("always") {
            let mut s = self.docker.create_image(
                Some(CreateImageOptions {
                    from_image: image,
                    platform: self.config.docker.platform.as_str(),
                    ..Default::default()
                }),
                None,
                None,
            );
            loop {
                let item = tokio::select! {
                    _ = cancellation.cancelled() => {
                        return Ok(Some(ExecutionTermination::Cancelled));
                    }
                    _ = tokio::time::sleep_until(deadline) => {
                        return Ok(Some(ExecutionTermination::TimedOut));
                    }
                    item = s.next() => item,
                };
                let Some(item) = item else {
                    break;
                };
                item.map_err(DockerFailure::image_pull)?;
            }
        }
        Ok(None)
    }

    async fn start_services(
        &self,
        services: &[ServiceSpec],
        network_name: &str,
        job_id: &str,
        resources: &runner_protocol::Resources,
        cancellation: &tokio_util::sync::CancellationToken,
        deadline: tokio::time::Instant,
    ) -> Result<Vec<String>> {
        let mut ids = Vec::new();
        for service in services {
            if service.name.trim().is_empty()
                || service.name.contains('/')
                || service.name.contains(':')
            {
                return Err(anyhow!("invalid service name: {}", service.name));
            }
            if service.image.trim().is_empty() || service.image.contains(char::is_whitespace) {
                return Err(anyhow!("invalid service image for {}", service.name));
            }
            if let Err(error) = self
                .pull(&service.image, cancellation, deadline)
                .await
                .and_then(|termination| {
                    termination.map_or(Ok(()), |reason| {
                        Err(anyhow!(match reason {
                            ExecutionTermination::Cancelled => "execution cancelled",
                            ExecutionTermination::TimedOut => "execution timed out",
                        }))
                    })
                })
            {
                self.remove_containers(&ids).await;
                return Err(error);
            }
            let id = match self
                .docker
                .create_container(
                    Some(CreateContainerOptions::<String> {
                        name: format!("ai-kodu-runner-service-{}-{}", service.name, Uuid::new_v4()),
                        platform: Some(self.config.docker.platform.clone()),
                    }),
                    Config {
                        image: Some(service.image.clone()),
                        cmd: (!service.command.is_empty()).then(|| service.command.clone()),
                        env: Some(
                            service
                                .environment
                                .iter()
                                .map(|(k, v)| format!("{k}={v}"))
                                .collect(),
                        ),
                        networking_config: Some(NetworkingConfig {
                            endpoints_config: HashMap::from([(
                                network_name.to_string(),
                                EndpointSettings {
                                    aliases: Some(vec![
                                        service
                                            .alias
                                            .clone()
                                            .unwrap_or_else(|| service.name.clone()),
                                    ]),
                                    ..Default::default()
                                },
                            )]),
                        }),
                        healthcheck: service.healthcheck.as_ref().map(|h| HealthConfig {
                            test: Some(
                                std::iter::once("CMD".to_string())
                                    .chain(h.command.clone())
                                    .collect(),
                            ),
                            ..Default::default()
                        }),
                        labels: Some(HashMap::from([
                            ("ai-kodu-runner.managed".into(), "true".into()),
                            ("ai-kodu-runner.runner_id".into(), self.config.runner_id()),
                            ("ai-kodu-runner.job_id".into(), job_id.to_string()),
                            ("ai-kodu-runner.service".into(), service.name.clone()),
                        ])),
                        host_config: Some(HostConfig {
                            memory: Some(resources.memory_mb * 1024 * 1024),
                            nano_cpus: Some((resources.cpu * 1_000_000_000.0) as i64),
                            pids_limit: Some(resources.pids),
                            // Some official service images (for example Redis,
                            // Postgres and MySQL) start as root and use setpriv
                            // to drop to their service user. Dropping every
                            // capability prevents that entrypoint transition.
                            // The application container keeps the stricter
                            // cap_drop=ALL profile below.
                            security_opt: Some(vec!["no-new-privileges:true".into()]),
                            auto_remove: Some(false),
                            ..Default::default()
                        }),
                        ..Default::default()
                    },
                )
                .await
            {
                Ok(container) => container.id,
                Err(error) => {
                    self.remove_containers(&ids).await;
                    return Err(error.into());
                }
            };
            if let Err(error) = self
                .docker
                .start_container(&id, None::<StartContainerOptions<String>>)
                .await
            {
                let _ = self.remove_container(&id).await;
                self.remove_containers(&ids).await;
                return Err(error.into());
            }
            ids.push(id);
        }
        for (service, id) in services.iter().zip(&ids) {
            if let Some(healthcheck) = &service.healthcheck {
                let health_deadline = tokio::time::Instant::now()
                    + std::time::Duration::from_secs(healthcheck.timeout_seconds);
                loop {
                    let result = self
                        .exec_command(
                            id,
                            &CommandSpec {
                                command: healthcheck.command.clone(),
                                working_directory: None,
                            },
                            cancellation,
                            deadline.min(health_deadline),
                        )
                        .await?;
                    if let Some(termination) = result.termination {
                        self.remove_containers(&ids).await;
                        return Err(anyhow!(match termination {
                            ExecutionTermination::Cancelled => "execution cancelled",
                            ExecutionTermination::TimedOut => "execution timed out",
                        }));
                    }
                    let status = result.status;
                    let output_stdout = result.stdout;
                    let output_stderr = result.stderr;
                    if status == 0 {
                        break;
                    }
                    if tokio::time::Instant::now() >= health_deadline {
                        self.remove_containers(&ids).await;
                        return Err(anyhow!(
                            "service {} did not become healthy: stdout: {}\nstderr: {}",
                            service.name,
                            String::from_utf8_lossy(&output_stdout),
                            String::from_utf8_lossy(&output_stderr)
                        ));
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                }
            }
        }
        Ok(ids)
    }

    async fn remove_container(&self, id: &str) -> Result<()> {
        self.docker
            .remove_container(
                id,
                Some(RemoveContainerOptions {
                    force: true,
                    ..Default::default()
                }),
            )
            .await?;
        Ok(())
    }

    async fn remove_containers(&self, ids: &[String]) {
        for id in ids {
            let _ = self.remove_container(id).await;
        }
    }
}

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
mod tests {
    use super::{
        DockerFailure, command_with_opencode_retry, headless_opencode_environment,
        redact_diagnostic_bytes, redact_diagnostic_value, runtime_tmpfs, safe_feedback_path,
    };
    use runner_protocol::JobSpec;

    #[test]
    fn docker_failure_exposes_stable_code_and_safe_failure_info() {
        let failure = DockerFailure::image_pull("registry token=secret");
        assert_eq!(failure.code, "image_pull_failed");
        assert_eq!(failure.phase, "image_pull");
        let info = failure.failure_info();
        assert_eq!(info.kind, runner_protocol::FailureKind::Infrastructure);
        assert_eq!(info.code, "image_pull_failed");
    }

    #[test]
    fn runtime_tmpfs_includes_writable_opencode_cache() {
        let mounts = runtime_tmpfs();
        let options = mounts
            .get("/home/opencode/.cache/opencode")
            .expect("OpenCode cache must be writable with a read-only root filesystem");

        assert!(options.contains("rw"));
        assert!(options.contains("uid=10001"));
        assert!(options.contains("gid=10001"));
        assert!(options.contains("mode=0700"));
    }

    #[test]
    fn runtime_tmpfs_includes_all_opencode_persistent_state() {
        let mounts = runtime_tmpfs();
        let options = mounts
            .get("/home/opencode/.local/share/opencode")
            .expect("OpenCode logs, snapshots, and future state must be writable");

        assert!(options.contains("rw"));
        assert!(options.contains("uid=10001"));
        assert!(options.contains("gid=10001"));
        assert!(options.contains("size=1g"));
        assert!(!mounts.contains_key("/home/opencode/.local/share/opencode/log"));
    }

    #[test]
    fn runtime_tmpfs_gives_provider_install_enough_space() {
        let mounts = runtime_tmpfs();
        let options = mounts
            .get("/home/opencode/.config/opencode")
            .expect("OpenCode config directory must be writable");

        assert!(options.contains("size=512m"));
    }

    #[test]
    fn wraps_generated_opencode_command_with_transient_server_retry() {
        let command = vec![
            "bash".into(),
            "-lc".into(),
            "opencode run task | tee .ai-kodu-runner/results/opencode.json; exit ${PIPESTATUS[0]}"
                .into(),
        ];

        let wrapped = command_with_opencode_retry(&command);

        assert_eq!(wrapped.len(), 3);
        assert!(wrapped[2].contains("Unexpected server error"));
        assert_eq!(
            wrapped[2]
                .matches("opencode run --print-logs --log-level INFO task")
                .count(),
            2
        );
    }

    #[test]
    fn leaves_non_opencode_commands_unchanged() {
        let command = vec!["bash".into(), "-lc".into(), "flutter test".into()];

        assert_eq!(command_with_opencode_retry(&command), command);
    }

    #[test]
    fn disables_title_agent_only_for_headless_opencode() {
        let opencode = vec!["bash".into(), "-lc".into(), "opencode run task".into()];
        let flutter = vec!["bash".into(), "-lc".into(), "flutter test".into()];

        let opencode_env = headless_opencode_environment(Vec::new(), &opencode);
        let flutter_env = headless_opencode_environment(Vec::new(), &flutter);

        assert!(
            opencode_env
                .iter()
                .any(|value| value.contains(r#""title":{"disable":true}"#))
        );
        assert!(
            opencode_env
                .iter()
                .any(|value| value == "OPENCODE_DISABLE_MODELS_FETCH=true")
        );
        assert!(flutter_env.is_empty());
    }

    #[test]
    fn redacts_nested_diagnostic_credentials() {
        let mut value = serde_json::json!({
            "workspace": { "token": "git-token", "username": "bot" },
            "provider": { "apiKey": "api-key" },
            "secrets": [{ "name": "MODEL_KEY", "value": "secret-value" }],
            "workflow": { "agent": { "command": ["curl", "--header", "secret-value"] } }
        });

        redact_diagnostic_value(&mut value);

        assert_eq!(value["workspace"]["token"], "<redacted>");
        assert_eq!(value["provider"]["apiKey"], "<redacted>");
        assert_eq!(value["secrets"][0]["value"], "<redacted>");
        assert_eq!(
            value["workflow"]["agent"]["command"][0],
            "<redacted: command may contain credentials>"
        );
        assert_eq!(value["workspace"]["username"], "bot");
    }

    #[test]
    fn diagnostic_output_redacts_known_secrets_and_is_bounded() {
        let spec = JobSpec::from_json(
            r#"{
                "api_version":"ai-kodu-runner.dev/v1alpha1","id":"diag","attempt":0,
                "executor":"docker","image":"example@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                "command":["true"],"working_directory":"/workspace",
                "workspace":{"kind":"local","path":"."},
                "resources":{"cpu":1.0,"memory_mb":128,"pids":32,"timeout_seconds":60},
                "network":{"mode":"none"},
                "secrets":[{"name":"MODEL_KEY","value":"super-secret"}]
            }"#,
        )
        .unwrap();
        let output = redact_diagnostic_bytes(&spec, b"before super-secret after");
        assert_eq!(
            String::from_utf8(output).unwrap(),
            "before <redacted> after"
        );
        let output = redact_diagnostic_bytes(&spec, &vec![b'x'; 70 * 1024]);
        assert!(output.len() < 70 * 1024);
        assert!(String::from_utf8_lossy(&output).contains("diagnostic output truncated"));
    }

    #[test]
    fn feedback_path_is_contained_and_creates_safe_parent() {
        let root = tempfile::tempdir().unwrap();
        let feedback = safe_feedback_path(root.path(), "/workspace/.runner/feedback.md").unwrap();

        assert_eq!(
            feedback,
            root.path()
                .canonicalize()
                .unwrap()
                .join(".runner/feedback.md")
        );
        assert!(root.path().join(".runner").is_dir());
    }

    #[cfg(unix)]
    #[test]
    fn feedback_path_rejects_symlink_parent() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), root.path().join(".runner")).unwrap();

        assert!(safe_feedback_path(root.path(), "/workspace/.runner/feedback.md").is_err());
    }
}

#[async_trait::async_trait]
impl Executor for DockerExecutor {
    fn capabilities(&self) -> ExecutorCapabilities {
        ExecutorCapabilities::new(["artifacts", "cancellation", "network", "streaming_logs"])
    }

    async fn doctor(&self) -> Result<DoctorReport> {
        self.docker.ping().await.context("Docker ping")?;
        let version = self.docker.version().await?;
        let test_name = format!("ai-kodu-runner-doctor-{}", Uuid::new_v4());
        let test_id = self
            .docker
            .create_container(
                Some(CreateContainerOptions::<String> {
                    name: test_name,
                    platform: Some(self.config.docker.platform.clone()),
                }),
                Config::<String> {
                    image: Some("alpine:3.20".into()),
                    cmd: Some(vec!["true".into()]),
                    ..Default::default()
                },
            )
            .await?
            .id;
        self.docker
            .remove_container(
                &test_id,
                Some(RemoveContainerOptions {
                    force: true,
                    ..Default::default()
                }),
            )
            .await?;
        Ok(DoctorReport {
            executor: "docker".into(),
            healthy: true,
            capabilities: self.capabilities(),
            checks: vec![
                DoctorCheck {
                    name: "docker_ping".into(),
                    healthy: true,
                    message: "Docker Engine responded to ping".into(),
                },
                DoctorCheck {
                    name: "docker_version".into(),
                    healthy: true,
                    message: version
                        .version
                        .unwrap_or_else(|| "Docker version unavailable".into()),
                },
                DoctorCheck {
                    name: "test_container".into(),
                    healthy: true,
                    message: "Docker can create and remove a test container".into(),
                },
            ],
        })
    }

    async fn run(
        &self,
        spec: JobSpec,
        cancel: Option<tokio_util::sync::CancellationToken>,
    ) -> Result<JobResult> {
        let cancellation = cancel.unwrap_or_default();
        self.begin_log_stream(&spec);
        self.set_log_phase(if spec.workflow.is_some() {
            "prepare"
        } else {
            "command"
        });
        info!(
            job_id = %spec.id,
            image = %spec.image,
            network = %spec.network.mode,
            platform = %self.config.docker.platform,
            "job execution started"
        );
        let resources = policy::validate(&spec, &self.config, false)?;
        let deadline =
            tokio::time::Instant::now() + std::time::Duration::from_secs(resources.timeout_seconds);
        if let Some(workflow) = spec.workflow.clone() {
            return self
                .run_workflow(spec, workflow, resources, cancellation, deadline)
                .await;
        }
        let journal = Journal::open(&self.config.work_dir.join("runner.db"))?;
        journal.transition(&spec.id, spec.attempt, state::State::Received)?;
        journal.transition(&spec.id, spec.attempt, state::State::Preparing)?;
        let prepared =
            workspace::prepare(&spec.workspace, &self.config, &cancellation, deadline).await?;
        if let Some(termination) = self.pull(&spec.image, &cancellation, deadline).await? {
            return Err(anyhow!(match termination {
                ExecutionTermination::Cancelled => "execution cancelled",
                ExecutionTermination::TimedOut => "execution timed out",
            }));
        }
        let network_name = format!("ai-kodu-runner-net-{}", Uuid::new_v4());
        let network = if spec.network.mode == "bridge" {
            Some(
                self.docker
                    .create_network(CreateNetworkOptions {
                        name: network_name.clone(),
                        check_duplicate: true,
                        driver: "bridge".into(),
                        internal: false,
                        attachable: false,
                        labels: HashMap::from([
                            ("ai-kodu-runner.managed".into(), "true".into()),
                            ("ai-kodu-runner.runner_id".into(), self.config.runner_id()),
                            ("ai-kodu-runner.job_id".into(), spec.id.clone()),
                        ]),
                        ..Default::default()
                    })
                    .await?
                    .id,
            )
        } else {
            None
        };
        let name = format!("ai-kodu-runner-job-{}", Uuid::new_v4());
        let env = headless_opencode_environment(
            policy::environment_for_job(&spec, &self.config)?,
            &spec.command,
        );
        let labels = HashMap::from([
            ("ai-kodu-runner.managed".into(), "true".into()),
            ("ai-kodu-runner.runner_id".into(), self.config.runner_id()),
            ("ai-kodu-runner.job_id".into(), spec.id.clone()),
            ("ai-kodu-runner.attempt".into(), spec.attempt.to_string()),
            (
                "ai-kodu-runner.expires_at".into(),
                (chrono::Utc::now() + chrono::Duration::seconds(resources.timeout_seconds as i64))
                    .to_rfc3339(),
            ),
        ]);
        let host = HostConfig {
            binds: Some(vec![format!(
                "{}:/workspace",
                prepared.dir.path().display()
            )]),
            network_mode: if network.is_some() {
                Some(network_name.clone())
            } else {
                Some("none".into())
            },
            memory: Some(resources.memory_mb * 1024 * 1024),
            nano_cpus: Some((resources.cpu * 1_000_000_000.0) as i64),
            pids_limit: Some(resources.pids),
            cap_drop: Some(vec!["ALL".into()]),
            security_opt: Some(vec!["no-new-privileges:true".into()]),
            readonly_rootfs: Some(!spec.writable_rootfs),
            tmpfs: Some(runtime_tmpfs()),
            auto_remove: Some(false),
            ..Default::default()
        };
        let id = match self
            .docker
            .create_container(
                Some(CreateContainerOptions {
                    name,
                    platform: Some(self.config.docker.platform.clone()),
                }),
                Config {
                    image: Some(spec.image.clone()),
                    cmd: Some(command_with_opencode_retry(&spec.command)),
                    working_dir: Some(spec.working_directory.clone()),
                    env: Some(env),
                    host_config: Some(host),
                    labels: Some(labels),
                    tty: Some(false),
                    open_stdin: Some(false),
                    ..Default::default()
                },
            )
            .await
        {
            Ok(container) => container.id,
            Err(error) => {
                if let Some(network) = &network {
                    let _ = self.docker.remove_network(network).await;
                }
                return Err(error.into());
            }
        };
        journal.transition(&spec.id, spec.attempt, state::State::Running)?;
        let started = Instant::now();
        if let Err(error) = self
            .docker
            .start_container(&id, None::<StartContainerOptions<String>>)
            .await
        {
            let _ = self
                .docker
                .remove_container(
                    &id,
                    Some(RemoveContainerOptions {
                        force: true,
                        ..Default::default()
                    }),
                )
                .await;
            if let Some(network) = &network {
                let _ = self.docker.remove_network(network).await;
            }
            return Err(error.into());
        }
        let mut logs = self.docker.logs(
            &id,
            Some(LogsOptions::<String> {
                follow: true,
                stdout: true,
                stderr: true,
                since: 0,
                until: 0,
                timestamps: false,
                tail: "all".into(),
            }),
        );
        let mut log_bytes = 0u64;
        let mut truncated = false;
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let mut cancellation_requested = false;
        let mut timeout_requested = false;
        loop {
            let item = tokio::select! {
                _ = cancellation.cancelled() => {
                    cancellation_requested = true;
                    let _ = self
                        .docker
                        .stop_container(
                            &id,
                            Some(StopContainerOptions { t: 5 }),
                        )
                        .await;
                    break;
                }
                _ = tokio::time::sleep_until(deadline) => {
                    timeout_requested = true;
                    let _ = self
                        .docker
                        .stop_container(
                            &id,
                            Some(StopContainerOptions { t: 5 }),
                        )
                        .await;
                    break;
                }
                item = logs.next() => item,
            };
            let Some(item) = item else {
                break;
            };
            match item? {
                LogOutput::StdOut { message } => {
                    if log_bytes < self.config.limits.max_log_bytes {
                        let remaining = self.config.limits.max_log_bytes - log_bytes;
                        let n = message.len().min(remaining as usize);
                        print!("{}", String::from_utf8_lossy(&message[..n]));
                        stdout.extend_from_slice(&message[..n]);
                        self.emit_log("stdout", &message[..n]);
                        log_bytes += n as u64;
                        if n < message.len() {
                            truncated = true;
                        }
                    } else {
                        truncated = true;
                    }
                }
                LogOutput::StdErr { message } => {
                    if log_bytes < self.config.limits.max_log_bytes {
                        let remaining = self.config.limits.max_log_bytes - log_bytes;
                        let n = message.len().min(remaining as usize);
                        eprint!("{}", String::from_utf8_lossy(&message[..n]));
                        stderr.extend_from_slice(&message[..n]);
                        self.emit_log("stderr", &message[..n]);
                        log_bytes += n as u64;
                        if n < message.len() {
                            truncated = true;
                        }
                    } else {
                        truncated = true;
                    }
                }
                _ => {}
            }
        }
        let status = match self.docker.inspect_container(&id, None).await {
            Ok(container) => container
                .state
                .and_then(|state| state.exit_code)
                .unwrap_or(-1),
            Err(e) => {
                let _ = self
                    .docker
                    .remove_container(
                        &id,
                        Some(RemoveContainerOptions {
                            force: true,
                            ..Default::default()
                        }),
                    )
                    .await;
                if let Some(n) = &network {
                    let _ = self.docker.remove_network(n).await;
                }
                return Err(anyhow!("Docker container inspect failed for {id}: {e}"));
            }
        };
        journal.transition(&spec.id, spec.attempt, state::State::Collecting)?;
        let cancelled = cancellation_requested || cancellation.is_cancelled();
        let timed_out =
            !cancelled && (timeout_requested || tokio::time::Instant::now() >= deadline);
        let mut final_exit_code = status;
        let final_status = if cancelled {
            "cancelled"
        } else if timed_out {
            "timed_out"
        } else if status == 0 {
            "completed"
        } else {
            "failed"
        };
        let final_status = if final_status == "completed" {
            match workspace::publish_git(
                &spec.workspace,
                prepared.dir.path(),
                &cancellation,
                deadline,
            )
            .await
            {
                Ok(()) => final_status,
                Err(error) => {
                    warn!(job_id=%spec.id, error=%error, "git publish failed");
                    stderr.extend_from_slice(format!("git publish failed: {error}\n").as_bytes());
                    final_exit_code = 1;
                    "failed"
                }
            }
        } else {
            final_status
        };
        let mut artifact_patterns = spec.artifacts.clone();
        if final_status != "completed" {
            write_failure_diagnostics(
                prepared.dir.path(),
                &spec,
                Some("command"),
                &stdout,
                &stderr,
            )?;
            artifact_patterns.push(".runner/diagnostics/**".into());
        }
        let export_dir = (!artifact_patterns.is_empty())
            .then(|| artifacts::destination(&self.config.work_dir, &spec.id, spec.attempt));
        let files = match &export_dir {
            Some(dir) => artifacts::export(
                prepared.dir.path(),
                &artifact_patterns,
                dir,
                self.config.limits.max_artifact_bytes,
                self.config.limits.max_artifact_files,
            )?
            .unwrap_or_default(),
            None => Vec::new(),
        };
        journal.transition(&spec.id, spec.attempt, state::State::from_str(final_status))?;
        journal.transition(&spec.id, spec.attempt, state::State::Destroying)?;
        let image_id = self
            .docker
            .inspect_container(&id, None)
            .await
            .ok()
            .and_then(|x| x.image);
        let _ = self
            .docker
            .remove_container(
                &id,
                Some(RemoveContainerOptions {
                    force: true,
                    ..Default::default()
                }),
            )
            .await;
        if let Some(n) = network {
            let _ = self.docker.remove_network(&n).await;
        }
        journal.transition(&spec.id, spec.attempt, state::State::Destroyed)?;
        info!(job_id=%spec.id, status=%final_status, exit_code=final_exit_code,"job finished");
        Ok(JobResult {
            job_id: spec.id,
            attempt: spec.attempt,
            status: final_status.into(),
            exit_code: (!cancelled && !timed_out).then_some(final_exit_code),
            started_at: chrono::Utc::now().to_rfc3339(),
            finished_at: chrono::Utc::now().to_rfc3339(),
            duration_ms: started.elapsed().as_millis(),
            log_truncated: truncated || self.dropped_log_chunks.load(Ordering::Relaxed) > 0,
            stdout: String::from_utf8_lossy(&stdout).into_owned(),
            stderr: String::from_utf8_lossy(&stderr).into_owned(),
            error_summary: runner_protocol::error_summary(
                final_status,
                &String::from_utf8_lossy(&stdout),
                &String::from_utf8_lossy(&stderr),
            ),
            failure: (final_status != "completed").then(|| FailureInfo {
                kind: if final_status == "cancelled" {
                    FailureKind::Cancellation
                } else if final_status == "timed_out" {
                    FailureKind::Timeout
                } else {
                    FailureKind::Execution
                },
                code: match final_status {
                    "cancelled" => "cancelled",
                    "timed_out" => "timeout",
                    _ => "command_failed",
                }
                .into(),
                message: match final_status {
                    "cancelled" => "execution cancelled",
                    "timed_out" => "execution timed out",
                    _ => "Docker command failed",
                }
                .into(),
            }),
            failed_phase: (final_status != "completed").then(|| "command".into()),
            artifacts: files,
            artifact_dir: export_dir.map(|path| path.to_string_lossy().into_owned()),
            sandbox: SandboxResult {
                executor: "docker".into(),
                container_id: id,
                image_id,
            },
        })
    }
    async fn cleanup(&self) -> Result<()> {
        let filters = HashMap::from([(
            "label".to_string(),
            vec![
                "ai-kodu-runner.managed=true".to_string(),
                format!("ai-kodu-runner.runner_id={}", self.config.runner_id()),
            ],
        )]);
        let items = self
            .docker
            .list_containers(Some(ListContainersOptions {
                all: true,
                filters,
                ..Default::default()
            }))
            .await?;
        let mut cleanup_error = None;
        let mut active_resources = false;
        for container in items {
            if container.state.as_deref() == Some("running") {
                // Manual cleanup must not interrupt a currently leased job.
                active_resources = true;
                continue;
            }
            if let Some(id) = container.id
                && let Err(error) = self
                    .docker
                    .remove_container(
                        &id,
                        Some(RemoveContainerOptions {
                            force: true,
                            ..Default::default()
                        }),
                    )
                    .await
            {
                cleanup_error.get_or_insert(error.to_string());
            }
        }
        let network_filters = HashMap::from([(
            "label".to_string(),
            vec![
                "ai-kodu-runner.managed=true".to_string(),
                format!("ai-kodu-runner.runner_id={}", self.config.runner_id()),
            ],
        )]);
        let networks = self
            .docker
            .list_networks(Some(ListNetworksOptions {
                filters: network_filters,
            }))
            .await?;
        for network in networks {
            if network
                .containers
                .as_ref()
                .is_some_and(|containers| !containers.is_empty())
            {
                active_resources = true;
                continue;
            }
            if let Some(id) = network.id
                && let Err(error) = self.docker.remove_network(&id).await
            {
                cleanup_error.get_or_insert(error.to_string());
            }
        }
        let journal = Journal::open(&self.config.work_dir.join("runner.db"))?;
        if let Some(error) = cleanup_error {
            bail!("cleanup incomplete: {error}")
        }
        if !active_resources {
            for (id, attempt) in journal.unfinished()? {
                journal.transition(&id, attempt, state::State::Destroying)?;
                journal.transition(&id, attempt, state::State::Destroyed)?;
            }
        }
        Ok(())
    }
}

impl DockerExecutor {
    async fn exec_command(
        &self,
        container_id: &str,
        command: &CommandSpec,
        cancellation: &tokio_util::sync::CancellationToken,
        deadline: tokio::time::Instant,
    ) -> Result<ExecResult> {
        let exec = tokio::select! {
            _ = cancellation.cancelled() => {
                let _ = self.stop_container(container_id).await;
                return Ok(ExecResult {
                    status: -1,
                    stdout: Vec::new(),
                    stderr: Vec::new(),
                    truncated: false,
                    termination: Some(ExecutionTermination::Cancelled),
                });
            }
            _ = tokio::time::sleep_until(deadline) => {
                let _ = self.stop_container(container_id).await;
                return Ok(ExecResult {
                    status: -1,
                    stdout: Vec::new(),
                    stderr: Vec::new(),
                    truncated: false,
                    termination: Some(ExecutionTermination::TimedOut),
                });
            }
            result = self.docker.create_exec(
                container_id,
                CreateExecOptions::<String> {
                    attach_stdout: Some(true),
                    attach_stderr: Some(true),
                    cmd: Some(command.command.clone()),
                    working_dir: command.working_directory.clone(),
                    ..Default::default()
                },
            ) => result?,
        };
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let mut truncated = false;
        let start = tokio::select! {
            _ = cancellation.cancelled() => {
                let _ = self.stop_container(container_id).await;
                return Ok(ExecResult {
                    status: -1,
                    stdout,
                    stderr,
                    truncated,
                    termination: Some(ExecutionTermination::Cancelled),
                });
            }
            _ = tokio::time::sleep_until(deadline) => {
                let _ = self.stop_container(container_id).await;
                return Ok(ExecResult {
                    status: -1,
                    stdout,
                    stderr,
                    truncated,
                    termination: Some(ExecutionTermination::TimedOut),
                });
            }
            result = self.docker.start_exec(
                &exec.id,
                Some(StartExecOptions {
                    detach: false,
                    tty: false,
                    output_capacity: None,
                }),
            ) => result?,
        };
        match start {
            StartExecResults::Attached { mut output, .. } => loop {
                let item = tokio::select! {
                    _ = cancellation.cancelled() => {
                        let _ = self.stop_container(container_id).await;
                        return Ok(ExecResult {
                            status: -1,
                            stdout,
                            stderr,
                            truncated,
                            termination: Some(ExecutionTermination::Cancelled),
                        });
                    }
                    _ = tokio::time::sleep_until(deadline) => {
                        let _ = self.stop_container(container_id).await;
                        return Ok(ExecResult {
                            status: -1,
                            stdout,
                            stderr,
                            truncated,
                            termination: Some(ExecutionTermination::TimedOut),
                        });
                    }
                    item = output.next() => item,
                };
                let Some(item) = item else {
                    break;
                };
                match item? {
                    LogOutput::StdOut { message } => {
                        print!("{}", String::from_utf8_lossy(&message));
                        self.emit_log("stdout", &message);
                        truncated |=
                            append_bounded(&mut stdout, &message, self.config.limits.max_log_bytes);
                    }
                    LogOutput::StdErr { message } => {
                        eprint!("{}", String::from_utf8_lossy(&message));
                        self.emit_log("stderr", &message);
                        truncated |=
                            append_bounded(&mut stderr, &message, self.config.limits.max_log_bytes);
                    }
                    _ => {}
                }
            },
            StartExecResults::Detached => {
                return Err(anyhow!("workflow exec unexpectedly detached"));
            }
        }
        let status = self
            .docker
            .inspect_exec(&exec.id)
            .await?
            .exit_code
            .unwrap_or(-1);
        Ok(ExecResult {
            status,
            stdout,
            stderr,
            truncated,
            termination: None,
        })
    }

    async fn stop_container(&self, id: &str) -> Result<()> {
        self.docker
            .stop_container(id, Some(StopContainerOptions { t: 5 }))
            .await?;
        Ok(())
    }

    async fn run_workflow(
        &self,
        spec: JobSpec,
        workflow: WorkflowSpec,
        resources: runner_protocol::Resources,
        cancellation: tokio_util::sync::CancellationToken,
        deadline: tokio::time::Instant,
    ) -> Result<JobResult> {
        let journal = Journal::open(&self.config.work_dir.join("runner.db"))?;
        journal.transition(&spec.id, spec.attempt, state::State::Received)?;
        journal.transition(&spec.id, spec.attempt, state::State::Preparing)?;
        let prepared =
            workspace::prepare(&spec.workspace, &self.config, &cancellation, deadline).await?;
        if let Some(termination) = self.pull(&spec.image, &cancellation, deadline).await? {
            return Err(anyhow!(match termination {
                ExecutionTermination::Cancelled => "execution cancelled",
                ExecutionTermination::TimedOut => "execution timed out",
            }));
        }
        let network_name = format!("ai-kodu-runner-net-{}", Uuid::new_v4());
        let network = if spec.network.mode == "bridge" {
            Some(
                self.docker
                    .create_network(CreateNetworkOptions {
                        name: network_name.clone(),
                        check_duplicate: true,
                        driver: "bridge".into(),
                        internal: false,
                        attachable: false,
                        labels: HashMap::from([
                            ("ai-kodu-runner.managed".into(), "true".into()),
                            ("ai-kodu-runner.runner_id".into(), self.config.runner_id()),
                            ("ai-kodu-runner.job_id".into(), spec.id.clone()),
                        ]),
                        ..Default::default()
                    })
                    .await?
                    .id,
            )
        } else {
            None
        };
        let env = policy::environment_for_job(&spec, &self.config)?;
        let service_ids = if let Some(network_name) = network.as_deref() {
            match self
                .start_services(
                    &workflow.services,
                    network_name,
                    &spec.id,
                    &resources,
                    &cancellation,
                    deadline,
                )
                .await
            {
                Ok(ids) => ids,
                Err(error) => {
                    if let Some(n) = &network {
                        let _ = self.docker.remove_network(n).await;
                    }
                    return Err(error);
                }
            }
        } else if workflow.services.is_empty() {
            Vec::new()
        } else {
            if let Some(n) = &network {
                let _ = self.docker.remove_network(n).await;
            }
            return Err(anyhow!("workflow services require bridge networking"));
        };
        let host = HostConfig {
            binds: Some(vec![format!(
                "{}:/workspace",
                prepared.dir.path().display()
            )]),
            network_mode: if network.is_some() {
                Some(network_name)
            } else {
                Some("none".into())
            },
            memory: Some(resources.memory_mb * 1024 * 1024),
            nano_cpus: Some((resources.cpu * 1_000_000_000.0) as i64),
            pids_limit: Some(resources.pids),
            cap_drop: Some(vec!["ALL".into()]),
            security_opt: Some(vec!["no-new-privileges:true".into()]),
            readonly_rootfs: Some(!spec.writable_rootfs),
            tmpfs: Some(runtime_tmpfs()),
            auto_remove: Some(false),
            ..Default::default()
        };
        let id = match self
            .docker
            .create_container(
                Some(CreateContainerOptions::<String> {
                    name: format!("ai-kodu-runner-job-{}", Uuid::new_v4()),
                    platform: Some(self.config.docker.platform.clone()),
                }),
                Config::<String> {
                    image: Some(spec.image.clone()),
                    cmd: Some(vec!["sleep".into(), "infinity".into()]),
                    env: Some(env),
                    host_config: Some(host),
                    labels: Some(HashMap::from([
                        ("ai-kodu-runner.managed".into(), "true".into()),
                        ("ai-kodu-runner.runner_id".into(), self.config.runner_id()),
                        ("ai-kodu-runner.job_id".into(), spec.id.clone()),
                        ("ai-kodu-runner.attempt".into(), spec.attempt.to_string()),
                    ])),
                    ..Default::default()
                },
            )
            .await
        {
            Ok(container) => container.id,
            Err(error) => {
                self.remove_containers(&service_ids).await;
                if let Some(network) = &network {
                    let _ = self.docker.remove_network(network).await;
                }
                return Err(error.into());
            }
        };
        if let Err(error) = self
            .docker
            .start_container(&id, None::<StartContainerOptions<String>>)
            .await
        {
            let _ = self.remove_container(&id).await;
            self.remove_containers(&service_ids).await;
            if let Some(network) = &network {
                let _ = self.docker.remove_network(network).await;
            }
            return Err(error.into());
        }
        let feedback = safe_feedback_path(prepared.dir.path(), &workflow.feedback_file)?;
        std::fs::write(&feedback, "No verifier feedback yet.\n")?;
        journal.transition(&spec.id, spec.attempt, state::State::Running)?;
        let started = Instant::now();
        let mut final_status = "failed";
        let mut setup_ok = true;
        let mut failed_phase: Option<String> = None;
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let mut truncated = false;
        let mut termination = None;
        for setup in &workflow.setup {
            if cancellation.is_cancelled() || tokio::time::Instant::now() >= deadline {
                termination = Some(if cancellation.is_cancelled() {
                    ExecutionTermination::Cancelled
                } else {
                    ExecutionTermination::TimedOut
                });
                failed_phase = Some("setup".into());
                break;
            }
            self.set_log_phase("setup");
            let result = self
                .exec_command(&id, setup, &cancellation, deadline)
                .await?;
            truncated |= result.truncated;
            truncated |= append_bounded(
                &mut stdout,
                &result.stdout,
                self.config.limits.max_log_bytes,
            );
            truncated |= append_bounded(
                &mut stderr,
                &result.stderr,
                self.config.limits.max_log_bytes,
            );
            if let Some(reason) = result.termination {
                termination = Some(reason);
                failed_phase = Some("setup".into());
                break;
            }
            if result.status != 0 {
                setup_ok = false;
                failed_phase = Some("setup".into());
            }
        }
        if termination.is_none() && !setup_ok {
            std::fs::write(&feedback, "Workflow setup command failed.\n")?;
        }
        if termination.is_none()
            && !cancellation.is_cancelled()
            && setup_ok
            && let Some(initialize) = &workflow.initialize
        {
            self.set_log_phase("initialize");
            info!(job_id=%spec.id, "workflow agent context initialization started");
            let result = self
                .exec_command(&id, initialize, &cancellation, deadline)
                .await?;
            truncated |= result.truncated;
            truncated |= append_bounded(
                &mut stdout,
                &result.stdout,
                self.config.limits.max_log_bytes,
            );
            truncated |= append_bounded(
                &mut stderr,
                &result.stderr,
                self.config.limits.max_log_bytes,
            );
            if let Some(reason) = result.termination {
                termination = Some(reason);
                failed_phase = Some("initialize".into());
            } else if result.status != 0 {
                setup_ok = false;
                failed_phase = Some("initialize".into());
                std::fs::write(
                    &feedback,
                    format!(
                        "Agent context initialization failed with exit code {}.\n{}",
                        result.status,
                        String::from_utf8_lossy(&result.stdout)
                    ),
                )?;
            }
        }
        for iteration in 1..=workflow.max_iterations {
            if !setup_ok || cancellation.is_cancelled() || termination.is_some() {
                break;
            }
            info!(job_id=%spec.id, iteration, "workflow agent started");
            self.set_log_phase("agent");
            let agent = self
                .exec_command(&id, &workflow.agent, &cancellation, deadline)
                .await?;
            truncated |= agent.truncated;
            truncated |=
                append_bounded(&mut stdout, &agent.stdout, self.config.limits.max_log_bytes);
            truncated |=
                append_bounded(&mut stderr, &agent.stderr, self.config.limits.max_log_bytes);
            if let Some(reason) = agent.termination {
                termination = Some(reason);
                failed_phase = Some("agent".into());
                break;
            }
            let mut all_passed = agent.status == 0;
            if agent.status != 0 {
                failed_phase = Some("agent".into());
            }
            let mut report = format!("Iteration {iteration} agent exit code: {}\n", agent.status);
            for verifier in &workflow.verifiers {
                self.set_log_phase("verifier");
                let command = CommandSpec {
                    command: verifier.command.clone(),
                    working_directory: verifier.working_directory.clone(),
                };
                let result = self
                    .exec_command(&id, &command, &cancellation, deadline)
                    .await?;
                truncated |= result.truncated;
                truncated |= append_bounded(
                    &mut stdout,
                    &result.stdout,
                    self.config.limits.max_log_bytes,
                );
                truncated |= append_bounded(
                    &mut stderr,
                    &result.stderr,
                    self.config.limits.max_log_bytes,
                );
                if let Some(reason) = result.termination {
                    termination = Some(reason);
                    failed_phase = Some("verifier".into());
                    break;
                }
                report.push_str(&format!(
                    "\nVerifier {} exit code: {}\nstdout:\n{}\nstderr:\n{}\n",
                    verifier.name,
                    result.status,
                    String::from_utf8_lossy(&result.stdout),
                    String::from_utf8_lossy(&result.stderr)
                ));
                if verifier.required && result.status != 0 {
                    all_passed = false;
                    failed_phase = Some("verifier".into());
                }
            }
            for verifier in &self.config.security.mandatory_verifiers {
                if termination.is_some() {
                    break;
                }
                self.set_log_phase("verifier");
                let result = self
                    .exec_command(&id, verifier, &cancellation, deadline)
                    .await?;
                truncated |= result.truncated;
                truncated |= append_bounded(
                    &mut stdout,
                    &result.stdout,
                    self.config.limits.max_log_bytes,
                );
                truncated |= append_bounded(
                    &mut stderr,
                    &result.stderr,
                    self.config.limits.max_log_bytes,
                );
                if let Some(reason) = result.termination {
                    termination = Some(reason);
                    failed_phase = Some("verifier".into());
                    break;
                }
                report.push_str(&format!(
                    "\nMandatory verifier {:?} exit code: {}\nstdout:\n{}\nstderr:\n{}\n",
                    verifier.command,
                    result.status,
                    String::from_utf8_lossy(&result.stdout),
                    String::from_utf8_lossy(&result.stderr)
                ));
                if result.status != 0 {
                    all_passed = false;
                    failed_phase = Some("verifier".into());
                }
            }
            if termination.is_none()
                && all_passed
                && let Some(publish) = &workflow.publish
            {
                self.set_log_phase("publish");
                let result = self
                    .exec_command(&id, publish, &cancellation, deadline)
                    .await?;
                truncated |= result.truncated;
                truncated |= append_bounded(
                    &mut stdout,
                    &result.stdout,
                    self.config.limits.max_log_bytes,
                );
                truncated |= append_bounded(
                    &mut stderr,
                    &result.stderr,
                    self.config.limits.max_log_bytes,
                );
                if let Some(reason) = result.termination {
                    termination = Some(reason);
                    failed_phase = Some("publish".into());
                } else {
                    all_passed = result.status == 0;
                }
                if !all_passed && termination.is_none() {
                    failed_phase = Some("publish".into());
                }
            }
            if termination.is_none() && all_passed {
                final_status = "completed";
                break;
            }
            std::fs::write(&feedback, report)?;
            if iteration == workflow.max_iterations {
                break;
            }
            info!(job_id=%spec.id, next_iteration=iteration + 1, "verifier feedback written");
        }
        if termination.is_none() && cancellation.is_cancelled() {
            termination = Some(ExecutionTermination::Cancelled);
        }
        if termination.is_none() && tokio::time::Instant::now() >= deadline {
            termination = Some(ExecutionTermination::TimedOut);
        }
        if let Some(reason) = termination {
            final_status = match reason {
                ExecutionTermination::Cancelled => "cancelled",
                ExecutionTermination::TimedOut => "timed_out",
            };
            failed_phase = Some(final_status.into());
            let _ = self
                .docker
                .stop_container(&id, Some(StopContainerOptions { t: 5 }))
                .await;
        }
        journal.transition(&spec.id, spec.attempt, state::State::Collecting)?;
        let mut artifact_patterns = spec.artifacts.clone();
        if final_status != "completed" {
            write_failure_diagnostics(
                prepared.dir.path(),
                &spec,
                failed_phase.as_deref(),
                &stdout,
                &stderr,
            )?;
            artifact_patterns.push(".runner/diagnostics/**".into());
        }
        let export_dir = (!artifact_patterns.is_empty())
            .then(|| artifacts::destination(&self.config.work_dir, &spec.id, spec.attempt));
        let files = match &export_dir {
            Some(dir) => artifacts::export(
                prepared.dir.path(),
                &artifact_patterns,
                dir,
                self.config.limits.max_artifact_bytes,
                self.config.limits.max_artifact_files,
            )?
            .unwrap_or_default(),
            None => Vec::new(),
        };
        journal.transition(&spec.id, spec.attempt, state::State::from_str(final_status))?;
        journal.transition(&spec.id, spec.attempt, state::State::Destroying)?;
        let image_id = self
            .docker
            .inspect_container(&id, None)
            .await
            .ok()
            .and_then(|x| x.image);
        let _ = self
            .docker
            .remove_container(
                &id,
                Some(RemoveContainerOptions {
                    force: true,
                    ..Default::default()
                }),
            )
            .await;
        self.remove_containers(&service_ids).await;
        if let Some(n) = network {
            let _ = self.docker.remove_network(&n).await;
        }
        journal.transition(&spec.id, spec.attempt, state::State::Destroyed)?;
        Ok(JobResult {
            job_id: spec.id,
            attempt: spec.attempt,
            status: final_status.into(),
            exit_code: (final_status != "cancelled" && final_status != "timed_out")
                .then_some(if final_status == "completed" { 0 } else { 1 }),
            started_at: chrono::Utc::now().to_rfc3339(),
            finished_at: chrono::Utc::now().to_rfc3339(),
            duration_ms: started.elapsed().as_millis(),
            log_truncated: truncated || self.dropped_log_chunks.load(Ordering::Relaxed) > 0,
            stdout: String::from_utf8_lossy(&stdout).into_owned(),
            stderr: String::from_utf8_lossy(&stderr).into_owned(),
            error_summary: runner_protocol::error_summary(
                final_status,
                &String::from_utf8_lossy(&stdout),
                &String::from_utf8_lossy(&stderr),
            ),
            failure: (final_status != "completed").then(|| FailureInfo {
                kind: match final_status {
                    "cancelled" => FailureKind::Cancellation,
                    "timed_out" => FailureKind::Timeout,
                    _ => FailureKind::Execution,
                },
                code: match final_status {
                    "cancelled" => "cancelled",
                    "timed_out" => "timeout",
                    _ => "workflow_failed",
                }
                .into(),
                message: match final_status {
                    "cancelled" => "execution cancelled",
                    "timed_out" => "execution timed out",
                    _ => "workflow failed",
                }
                .into(),
            }),
            failed_phase: (final_status != "completed")
                .then(|| failed_phase.unwrap_or_else(|| "workflow".into())),
            artifacts: files,
            artifact_dir: export_dir.map(|path| path.to_string_lossy().into_owned()),
            sandbox: SandboxResult {
                executor: "docker".into(),
                container_id: id,
                image_id,
            },
        })
    }
}
