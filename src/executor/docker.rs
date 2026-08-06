use super::Executor;
use crate::{
    artifacts,
    config::RunnerConfig,
    job::{CommandSpec, JobResult, JobSpec, SandboxResult, WorkflowSpec},
    journal::Journal,
    policy, workspace,
};
use anyhow::{Context, Result, anyhow};
use bollard::container::{
    Config, CreateContainerOptions, LogOutput, LogsOptions, RemoveContainerOptions,
    StartContainerOptions,
};
use bollard::exec::{CreateExecOptions, StartExecOptions, StartExecResults};
use bollard::image::CreateImageOptions;
use bollard::models::HostConfig;
use bollard::network::CreateNetworkOptions;
use bollard::{API_DEFAULT_VERSION, Docker};
use futures_util::StreamExt;
use std::{collections::HashMap, time::Instant};
use tracing::info;
use uuid::Uuid;

pub struct DockerExecutor {
    config: RunnerConfig,
    docker: Docker,
}
impl DockerExecutor {
    pub fn new(config: RunnerConfig) -> Result<Self> {
        let docker =
            Docker::connect_with_local_defaults().context("connect to Docker Engine/Desktop")?;
        Ok(Self { config, docker })
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
                    platform: None,
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
                    ..Default::default()
                }),
                None,
                None,
            );
            while s.next().await.is_some() {}
        }
        Ok(())
    }
}
#[async_trait::async_trait]
impl Executor for DockerExecutor {
    async fn run(
        &self,
        spec: JobSpec,
        cancel: Option<tokio_util::sync::CancellationToken>,
    ) -> Result<JobResult> {
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
        let env = policy::environment_for_job(&spec, &self.config)?;
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
            tmpfs: Some(HashMap::from([(
                "/tmp".into(),
                "rw,noexec,nosuid,size=256m".into(),
            )])),
            auto_remove: Some(false),
            ..Default::default()
        };
        let id = self
            .docker
            .create_container(
                Some(CreateContainerOptions {
                    name,
                    platform: None,
                }),
                Config {
                    image: Some(spec.image.clone()),
                    cmd: Some(spec.command.clone()),
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
        while let Some(item) = logs.next().await {
            match item? {
                LogOutput::StdOut { message } | LogOutput::StdErr { message } => {
                    if log_bytes < self.config.limits.max_log_bytes {
                        let remaining = self.config.limits.max_log_bytes - log_bytes;
                        let n = message.len().min(remaining as usize);
                        print!("{}", String::from_utf8_lossy(&message[..n]));
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
        let export_dir = (!spec.artifacts.is_empty())
            .then(|| artifacts::destination(&self.config.work_dir, &spec.id, spec.attempt));
        let files = match &export_dir {
            Some(dir) => artifacts::export(
                prepared.dir.path(),
                &spec.artifacts,
                dir,
                self.config.limits.max_artifact_bytes,
            )?
            .unwrap_or_default(),
            None => Vec::new(),
        };
        let final_status = if status == 0 { "completed" } else { "failed" };
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
        info!(job_id=%spec.id,exit_code=status,"job finished");
        Ok(JobResult {
            job_id: spec.id,
            attempt: spec.attempt,
            status: final_status.into(),
            exit_code: Some(status),
            started_at: chrono::Utc::now().to_rfc3339(),
            finished_at: chrono::Utc::now().to_rfc3339(),
            duration_ms: started.elapsed().as_millis(),
            log_truncated: truncated,
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
    ) -> Result<(i64, Vec<u8>)> {
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
        let mut output_bytes = Vec::new();
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
                        LogOutput::StdOut { message } | LogOutput::StdErr { message } => {
                            print!("{}", String::from_utf8_lossy(&message));
                            output_bytes.extend_from_slice(&message);
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
        Ok((status, output_bytes))
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
            tmpfs: Some(HashMap::from([(
                String::from("/tmp"),
                String::from("rw,noexec,nosuid,size=256m"),
            )])),
            auto_remove: Some(false),
            ..Default::default()
        };
        let id = self
            .docker
            .create_container(
                Some(CreateContainerOptions::<String> {
                    name: format!("omniroute-job-{}", Uuid::new_v4()),
                    platform: None,
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
        for setup in &workflow.setup {
            let (status, _) = self.exec_command(&id, setup).await?;
            if status != 0 {
                setup_ok = false;
            }
        }
        if !setup_ok {
            std::fs::write(&feedback, "Workflow setup command failed.\n")?;
        }
        if setup_ok && let Some(initialize) = &workflow.initialize {
            info!(job_id=%spec.id, "workflow agent context initialization started");
            let (status, output) = self.exec_command(&id, initialize).await?;
            if status != 0 {
                setup_ok = false;
                std::fs::write(
                    &feedback,
                    format!(
                        "Agent context initialization failed with exit code {status}.\n{}",
                        String::from_utf8_lossy(&output)
                    ),
                )?;
            }
        }
        for iteration in 1..=workflow.max_iterations {
            if !setup_ok {
                break;
            }
            info!(job_id=%spec.id, iteration, "workflow agent started");
            let (agent_status, _) = self.exec_command(&id, &workflow.agent).await?;
            let mut all_passed = agent_status == 0;
            let mut report = format!("Iteration {iteration} agent exit code: {agent_status}\n");
            for verifier in &workflow.verifiers {
                let command = CommandSpec {
                    command: verifier.command.clone(),
                    working_directory: verifier.working_directory.clone(),
                };
                let (status, output) = self.exec_command(&id, &command).await?;
                report.push_str(&format!(
                    "\nVerifier {} exit code: {status}\n{}\n",
                    verifier.name,
                    String::from_utf8_lossy(&output)
                ));
                if verifier.required && status != 0 {
                    all_passed = false;
                }
            }
            if all_passed && let Some(publish) = &workflow.publish {
                let (publish_status, _) = self.exec_command(&id, publish).await?;
                all_passed = publish_status == 0;
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
        let export_dir = (!spec.artifacts.is_empty())
            .then(|| artifacts::destination(&self.config.work_dir, &spec.id, spec.attempt));
        let files = match &export_dir {
            Some(dir) => artifacts::export(
                prepared.dir.path(),
                &spec.artifacts,
                dir,
                self.config.limits.max_artifact_bytes,
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
        Ok(JobResult {
            job_id: spec.id,
            attempt: spec.attempt,
            status: final_status.into(),
            exit_code: Some(if final_status == "completed" { 0 } else { 1 }),
            started_at: chrono::Utc::now().to_rfc3339(),
            finished_at: chrono::Utc::now().to_rfc3339(),
            duration_ms: started.elapsed().as_millis(),
            log_truncated: false,
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
