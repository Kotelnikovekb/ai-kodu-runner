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
use super::Executor;
use crate::{
    artifacts,
    config::{RunnerConfig, platform_from_env, validate_platform},
    job::{CommandSpec, JobResult, JobSpec, LogChunk, SandboxResult, ServiceSpec, WorkflowSpec},
    journal::Journal,
    policy, workspace,
};
use anyhow::{Context, Result, anyhow};
use bollard::container::{
    Config, CreateContainerOptions, LogOutput, LogsOptions, NetworkingConfig,
    RemoveContainerOptions, StartContainerOptions,
};
use bollard::exec::{CreateExecOptions, StartExecOptions, StartExecResults};
use bollard::image::CreateImageOptions;
use bollard::models::{EndpointSettings, HealthConfig, HostConfig};
use bollard::network::CreateNetworkOptions;
use bollard::{API_DEFAULT_VERSION, Docker};
use futures_util::StreamExt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::{
    collections::HashMap,
    fs,
    path::Path,
    sync::{Arc, Mutex},
    time::Instant,
};

fn redact_diagnostic_value(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(object) => {
            for (key, value) in object {
                let normalized = key.to_ascii_lowercase();
                if normalized == "secrets" {
                    if let serde_json::Value::Array(secrets) = value {
                        for secret in secrets.iter_mut() {
                            if let Some(value) = secret.get_mut("value") {
                                *value = serde_json::Value::String("<redacted>".into());
                            }
                        }
                    }
                    redact_diagnostic_value(value);
                } else if matches!(
                    normalized.as_str(),
                    "token" | "password" | "secret" | "apikey" | "api_key" | "authorization"
                ) {
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
    if let Some(command) = job.get_mut("command") {
        *command = serde_json::json!(["<redacted: command may contain credentials>"]);
    }
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
    fs::write(dir.join("stdout.jsonl"), stdout)?;
    fs::write(dir.join("stderr.log"), stderr)?;
    for (source, target) in [
        ("prompt.md", "prompt.md"),
        ("AGENTS.md", "AGENTS.md"),
        (".runner/feedback.md", "feedback.md"),
    ] {
        let source = workspace.join(source);
        if source.is_file() {
            fs::copy(source, dir.join(target))?;
        }
    }
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
        || !command[2].contains(".omniroute/results/opencode.json")
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
if [ "$runner_rc" -ne 0 ] && grep -Fq 'Unexpected server error' .omniroute/results/opencode.json 2>/dev/null; then
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
        let test = format!("omniroute-doctor-{}", Uuid::new_v4());
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
    async fn pull(&self, image: &str) -> Result<()> {
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
            while s.next().await.is_some() {}
        }
        Ok(())
    }

    async fn start_services(
        &self,
        services: &[ServiceSpec],
        network_name: &str,
        job_id: &str,
        resources: &crate::job::Resources,
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
            if let Err(error) = self.pull(&service.image).await {
                self.remove_containers(&ids).await;
                return Err(error);
            }
            let id = match self
                .docker
                .create_container(
                    Some(CreateContainerOptions::<String> {
                        name: format!("omniroute-service-{}-{}", service.name, Uuid::new_v4()),
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
                            ("omniroute.managed".into(), "true".into()),
                            ("omniroute.runner_id".into(), self.config.runner_id()),
                            ("omniroute.job_id".into(), job_id.to_string()),
                            ("omniroute.service".into(), service.name.clone()),
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
                let deadline = tokio::time::Instant::now()
                    + std::time::Duration::from_secs(healthcheck.timeout_seconds);
                loop {
                    let (status, output_stdout, output_stderr) = self
                        .exec_command(
                            id,
                            &CommandSpec {
                                command: healthcheck.command.clone(),
                                working_directory: None,
                            },
                        )
                        .await?;
                    if status == 0 {
                        break;
                    }
                    if tokio::time::Instant::now() >= deadline {
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
mod tests {
    use super::{
        command_with_opencode_retry, headless_opencode_environment, redact_diagnostic_value,
        runtime_tmpfs,
    };

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
            "opencode run task | tee .omniroute/results/opencode.json; exit ${PIPESTATUS[0]}"
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
            "secrets": [{ "name": "MODEL_KEY", "value": "secret-value" }]
        });

        redact_diagnostic_value(&mut value);

        assert_eq!(value["workspace"]["token"], "<redacted>");
        assert_eq!(value["provider"]["apiKey"], "<redacted>");
        assert_eq!(value["secrets"][0]["value"], "<redacted>");
        assert_eq!(value["workspace"]["username"], "bot");
    }
}
#[async_trait::async_trait]
impl Executor for DockerExecutor {
    async fn run(
        &self,
        spec: JobSpec,
        cancel: Option<tokio_util::sync::CancellationToken>,
    ) -> Result<JobResult> {
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
        if let Some(workflow) = spec.workflow.clone() {
            return self.run_workflow(spec, workflow, resources).await;
        }
        let journal = Journal::open(&self.config.work_dir.join("runner.db"))?;
        journal.transition(&spec.id, spec.attempt, crate::state::State::Received)?;
        journal.transition(&spec.id, spec.attempt, crate::state::State::Preparing)?;
        let prepared = workspace::prepare(&spec.workspace, &self.config).await?;
        self.pull(&spec.image).await?;
        let network_name = format!("omniroute-net-{}", Uuid::new_v4());
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
                            ("omniroute.managed".into(), "true".into()),
                            ("omniroute.runner_id".into(), self.config.runner_id()),
                            ("omniroute.job_id".into(), spec.id.clone()),
                        ]),
                        ..Default::default()
                    })
                    .await?
                    .id,
            )
        } else {
            None
        };
        let name = format!("omniroute-job-{}", Uuid::new_v4());
        let env = headless_opencode_environment(
            policy::environment_for_job(&spec, &self.config)?,
            &spec.command,
        );
        let labels = HashMap::from([
            ("omniroute.managed".into(), "true".into()),
            ("omniroute.runner_id".into(), self.config.runner_id()),
            ("omniroute.job_id".into(), spec.id.clone()),
            ("omniroute.attempt".into(), spec.attempt.to_string()),
            (
                "omniroute.expires_at".into(),
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
        let id = self
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
            .await?
            .id;
        journal.transition(&spec.id, spec.attempt, crate::state::State::Running)?;
        let started = Instant::now();
        self.docker
            .start_container(&id, None::<StartContainerOptions<String>>)
            .await?;
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
        while let Some(item) = logs.next().await {
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
        journal.transition(&spec.id, spec.attempt, crate::state::State::Collecting)?;
        if let Some(token) = cancel
            && token.is_cancelled()
        {
            journal.transition(&spec.id, spec.attempt, crate::state::State::Cancelled)?;
        }
        let mut final_exit_code = status;
        let final_status = if status == 0 { "completed" } else { "failed" };
        let final_status = if final_status == "completed" {
            match crate::workspace::publish_git(&spec.workspace, prepared.dir.path()) {
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
        journal.transition(
            &spec.id,
            spec.attempt,
            crate::state::State::from_str(final_status),
        )?;
        journal.transition(&spec.id, spec.attempt, crate::state::State::Destroying)?;
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
        journal.transition(&spec.id, spec.attempt, crate::state::State::Destroyed)?;
        info!(job_id=%spec.id, status=%final_status, exit_code=final_exit_code,"job finished");
        Ok(JobResult {
            job_id: spec.id,
            attempt: spec.attempt,
            status: final_status.into(),
            exit_code: Some(final_exit_code),
            started_at: chrono::Utc::now().to_rfc3339(),
            finished_at: chrono::Utc::now().to_rfc3339(),
            duration_ms: started.elapsed().as_millis(),
            log_truncated: truncated,
            stdout: String::from_utf8_lossy(&stdout).into_owned(),
            stderr: String::from_utf8_lossy(&stderr).into_owned(),
            error_summary: crate::job::error_summary(
                final_status,
                &String::from_utf8_lossy(&stdout),
                &String::from_utf8_lossy(&stderr),
            ),
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
}

impl DockerExecutor {
    async fn exec_command(
        &self,
        container_id: &str,
        command: &CommandSpec,
    ) -> Result<(i64, Vec<u8>, Vec<u8>)> {
        let exec = self
            .docker
            .create_exec(
                container_id,
                CreateExecOptions::<String> {
                    attach_stdout: Some(true),
                    attach_stderr: Some(true),
                    cmd: Some(command.command.clone()),
                    working_dir: command.working_directory.clone(),
                    ..Default::default()
                },
            )
            .await?;
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        match self
            .docker
            .start_exec(
                &exec.id,
                Some(StartExecOptions {
                    detach: false,
                    tty: false,
                    output_capacity: None,
                }),
            )
            .await?
        {
            StartExecResults::Attached { mut output, .. } => {
                while let Some(item) = output.next().await {
                    match item? {
                        LogOutput::StdOut { message } => {
                            print!("{}", String::from_utf8_lossy(&message));
                            self.emit_log("stdout", &message);
                            stdout.extend_from_slice(&message);
                        }
                        LogOutput::StdErr { message } => {
                            eprint!("{}", String::from_utf8_lossy(&message));
                            self.emit_log("stderr", &message);
                            stderr.extend_from_slice(&message);
                        }
                        _ => {}
                    }
                }
            }
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
        Ok((status, stdout, stderr))
    }

    async fn run_workflow(
        &self,
        spec: JobSpec,
        workflow: WorkflowSpec,
        resources: crate::job::Resources,
    ) -> Result<JobResult> {
        let journal = Journal::open(&self.config.work_dir.join("runner.db"))?;
        journal.transition(&spec.id, spec.attempt, crate::state::State::Received)?;
        journal.transition(&spec.id, spec.attempt, crate::state::State::Preparing)?;
        let prepared = workspace::prepare(&spec.workspace, &self.config).await?;
        self.pull(&spec.image).await?;
        let network_name = format!("omniroute-net-{}", Uuid::new_v4());
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
                            ("omniroute.managed".into(), "true".into()),
                            ("omniroute.runner_id".into(), self.config.runner_id()),
                            ("omniroute.job_id".into(), spec.id.clone()),
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
                .start_services(&workflow.services, network_name, &spec.id, &resources)
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
        let id = self
            .docker
            .create_container(
                Some(CreateContainerOptions::<String> {
                    name: format!("omniroute-job-{}", Uuid::new_v4()),
                    platform: Some(self.config.docker.platform.clone()),
                }),
                Config::<String> {
                    image: Some(spec.image.clone()),
                    cmd: Some(vec!["sleep".into(), "infinity".into()]),
                    env: Some(env),
                    host_config: Some(host),
                    labels: Some(HashMap::from([
                        ("omniroute.managed".into(), "true".into()),
                        ("omniroute.runner_id".into(), self.config.runner_id()),
                        ("omniroute.job_id".into(), spec.id.clone()),
                        ("omniroute.attempt".into(), spec.attempt.to_string()),
                    ])),
                    ..Default::default()
                },
            )
            .await?
            .id;
        self.docker
            .start_container(&id, None::<StartContainerOptions<String>>)
            .await?;
        let feedback = prepared.dir.path().join(
            workflow
                .feedback_file
                .trim_start_matches("/workspace/")
                .trim_start_matches('/'),
        );
        if let Some(parent) = feedback.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&feedback, "No verifier feedback yet.\n")?;
        journal.transition(&spec.id, spec.attempt, crate::state::State::Running)?;
        let started = Instant::now();
        let mut final_status = "failed";
        let mut setup_ok = true;
        let mut failed_phase: Option<String> = None;
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        for setup in &workflow.setup {
            self.set_log_phase("setup");
            let (status, command_stdout, command_stderr) = self.exec_command(&id, setup).await?;
            stdout.extend_from_slice(&command_stdout);
            stderr.extend_from_slice(&command_stderr);
            if status != 0 {
                setup_ok = false;
                failed_phase = Some("setup".into());
            }
        }
        if !setup_ok {
            std::fs::write(&feedback, "Workflow setup command failed.\n")?;
        }
        if setup_ok && let Some(initialize) = &workflow.initialize {
            self.set_log_phase("initialize");
            info!(job_id=%spec.id, "workflow agent context initialization started");
            let (status, output_stdout, output_stderr) = self.exec_command(&id, initialize).await?;
            stdout.extend_from_slice(&output_stdout);
            stderr.extend_from_slice(&output_stderr);
            if status != 0 {
                setup_ok = false;
                failed_phase = Some("initialize".into());
                std::fs::write(
                    &feedback,
                    format!(
                        "Agent context initialization failed with exit code {status}.\n{}",
                        String::from_utf8_lossy(&output_stdout)
                    ),
                )?;
            }
        }
        for iteration in 1..=workflow.max_iterations {
            if !setup_ok {
                break;
            }
            info!(job_id=%spec.id, iteration, "workflow agent started");
            self.set_log_phase("agent");
            let (agent_status, agent_stdout, agent_stderr) =
                self.exec_command(&id, &workflow.agent).await?;
            stdout.extend_from_slice(&agent_stdout);
            stderr.extend_from_slice(&agent_stderr);
            let mut all_passed = agent_status == 0;
            if agent_status != 0 {
                failed_phase = Some("agent".into());
            }
            let mut report = format!("Iteration {iteration} agent exit code: {agent_status}\n");
            for verifier in &workflow.verifiers {
                self.set_log_phase("verifier");
                let command = CommandSpec {
                    command: verifier.command.clone(),
                    working_directory: verifier.working_directory.clone(),
                };
                let (status, output_stdout, output_stderr) =
                    self.exec_command(&id, &command).await?;
                stdout.extend_from_slice(&output_stdout);
                stderr.extend_from_slice(&output_stderr);
                report.push_str(&format!(
                    "\nVerifier {} exit code: {status}\nstdout:\n{}\nstderr:\n{}\n",
                    verifier.name,
                    String::from_utf8_lossy(&output_stdout),
                    String::from_utf8_lossy(&output_stderr)
                ));
                if verifier.required && status != 0 {
                    all_passed = false;
                    failed_phase = Some("verifier".into());
                }
            }
            for verifier in &self.config.security.mandatory_verifiers {
                self.set_log_phase("verifier");
                let (status, output_stdout, output_stderr) =
                    self.exec_command(&id, verifier).await?;
                stdout.extend_from_slice(&output_stdout);
                stderr.extend_from_slice(&output_stderr);
                report.push_str(&format!(
                    "\nMandatory verifier {:?} exit code: {status}\nstdout:\n{}\nstderr:\n{}\n",
                    verifier.command,
                    String::from_utf8_lossy(&output_stdout),
                    String::from_utf8_lossy(&output_stderr)
                ));
                if status != 0 {
                    all_passed = false;
                    failed_phase = Some("verifier".into());
                }
            }
            if all_passed && let Some(publish) = &workflow.publish {
                self.set_log_phase("publish");
                let (publish_status, publish_stdout, publish_stderr) =
                    self.exec_command(&id, publish).await?;
                stdout.extend_from_slice(&publish_stdout);
                stderr.extend_from_slice(&publish_stderr);
                all_passed = publish_status == 0;
                if publish_status != 0 {
                    failed_phase = Some("publish".into());
                }
            }
            if all_passed {
                final_status = "completed";
                break;
            }
            std::fs::write(&feedback, report)?;
            if iteration == workflow.max_iterations {
                break;
            }
            info!(job_id=%spec.id, next_iteration=iteration + 1, "verifier feedback written");
        }
        journal.transition(&spec.id, spec.attempt, crate::state::State::Collecting)?;
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
        journal.transition(
            &spec.id,
            spec.attempt,
            crate::state::State::from_str(final_status),
        )?;
        journal.transition(&spec.id, spec.attempt, crate::state::State::Destroying)?;
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
        journal.transition(&spec.id, spec.attempt, crate::state::State::Destroyed)?;
        Ok(JobResult {
            job_id: spec.id,
            attempt: spec.attempt,
            status: final_status.into(),
            exit_code: Some(if final_status == "completed" { 0 } else { 1 }),
            started_at: chrono::Utc::now().to_rfc3339(),
            finished_at: chrono::Utc::now().to_rfc3339(),
            duration_ms: started.elapsed().as_millis(),
            log_truncated: false,
            stdout: String::from_utf8_lossy(&stdout).into_owned(),
            stderr: String::from_utf8_lossy(&stderr).into_owned(),
            error_summary: crate::job::error_summary(
                final_status,
                &String::from_utf8_lossy(&stdout),
                &String::from_utf8_lossy(&stderr),
            ),
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
