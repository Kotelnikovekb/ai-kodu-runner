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
use anyhow::{Context, Result};
use runner_protocol::CommandSpec;
use serde::Deserialize;
use std::{
    env, fs,
    path::{Path, PathBuf},
};
use uuid::Uuid;

#[derive(Debug, Clone, Deserialize)]
pub struct RunnerConfig {
    pub server_url: Option<String>,
    pub runner_token: Option<String>,
    pub name: Option<String>,
    #[serde(skip)]
    runner_id: String,
    pub work_dir: PathBuf,
    #[allow(dead_code)]
    pub concurrency: Option<usize>,
    pub docker: DockerConfig,
    pub limits: Limits,
    pub security: SecurityConfig,
}
#[derive(Debug, Clone, Deserialize)]
pub struct DockerConfig {
    #[allow(dead_code)]
    pub host: Option<String>,
    pub pull_policy: Option<String>,
    #[serde(default = "default_platform")]
    pub platform: String,
}
#[derive(Debug, Clone, Deserialize)]
pub struct Limits {
    pub max_cpu: f64,
    pub max_memory_mb: i64,
    pub max_pids: i64,
    pub max_timeout_seconds: u64,
    pub max_log_bytes: u64,
    pub max_artifact_bytes: u64,
    #[serde(default = "default_max_artifact_files")]
    pub max_artifact_files: usize,
    #[serde(default = "default_max_workspace_bytes")]
    pub max_workspace_bytes: u64,
    #[serde(default = "default_max_workspace_files")]
    pub max_workspace_files: usize,
    #[serde(default = "default_max_archive_download_bytes")]
    pub max_archive_download_bytes: u64,
}

fn default_max_artifact_files() -> usize {
    10_000
}

fn default_max_workspace_bytes() -> u64 {
    1024 * 1024 * 1024
}

fn default_max_workspace_files() -> usize {
    100_000
}

fn default_max_archive_download_bytes() -> u64 {
    512 * 1024 * 1024
}

pub fn default_platform() -> String {
    "linux/amd64".into()
}

pub fn platform_from_env() -> String {
    env::var("RUNNER_PLATFORM").unwrap_or_else(|_| default_platform())
}

pub fn validate_platform(platform: &str) -> Result<()> {
    let parts: Vec<_> = platform.split('/').collect();
    if !(parts.len() == 2 || parts.len() == 3)
        || parts
            .iter()
            .any(|part| part.is_empty() || part.chars().any(char::is_whitespace))
    {
        anyhow::bail!(
            "invalid Docker platform {platform:?}; expected os/architecture or os/architecture/variant"
        );
    }
    Ok(())
}
#[derive(Debug, Clone, Deserialize)]
pub struct SecurityConfig {
    pub allow_network: bool,
    #[allow(dead_code)]
    pub allow_privileged: bool,
    #[allow(dead_code)]
    pub allow_host_mounts: bool,
    pub allowed_environment: Vec<String>,
    #[serde(default)]
    pub mandatory_verifiers: Vec<CommandSpec>,
}

impl RunnerConfig {
    pub fn load(path: &Path) -> Result<Self> {
        let mut config: Self = toml::from_str(
            &fs::read_to_string(path).with_context(|| format!("read config {}", path.display()))?,
        )
        .context("parse runner.toml")?;
        if let Ok(platform) = env::var("RUNNER_PLATFORM") {
            config.docker.platform = platform;
        }
        config.runner_id = config
            .name
            .clone()
            .unwrap_or_else(|| Uuid::new_v4().to_string());
        validate_platform(&config.docker.platform)?;
        Ok(config)
    }
    pub fn default_local() -> Self {
        Self {
            server_url: None,
            runner_token: None,
            name: Some("local".into()),
            runner_id: "local".into(),
            work_dir: env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            concurrency: Some(1),
            docker: DockerConfig {
                host: Some("auto".into()),
                pull_policy: Some("if-not-present".into()),
                platform: platform_from_env(),
            },
            limits: Limits {
                max_cpu: 4.0,
                max_memory_mb: 8192,
                max_pids: 512,
                max_timeout_seconds: 3600,
                max_log_bytes: 50 * 1024 * 1024,
                max_artifact_bytes: 500 * 1024 * 1024,
                max_artifact_files: default_max_artifact_files(),
                max_workspace_bytes: default_max_workspace_bytes(),
                max_workspace_files: default_max_workspace_files(),
                max_archive_download_bytes: default_max_archive_download_bytes(),
            },
            security: SecurityConfig {
                allow_network: true,
                allow_privileged: false,
                allow_host_mounts: false,
                allowed_environment: vec![],
                mandatory_verifiers: vec![],
            },
        }
    }
    pub fn runner_id(&self) -> String {
        self.runner_id.clone()
    }
    pub fn resolve_token(&self) -> Result<Option<String>> {
        self.runner_token
            .as_deref()
            .map(|v| {
                v.strip_prefix("env:").map_or_else(
                    || Ok(v.to_owned()),
                    |n| {
                        env::var(n)
                            .with_context(|| format!("missing secret environment variable {n}"))
                    },
                )
            })
            .transpose()
    }
}

#[cfg(test)]
mod tests {
    use super::{default_platform, validate_platform};

    #[test]
    fn defaults_to_linux_amd64() {
        assert_eq!(default_platform(), "linux/amd64");
    }

    #[test]
    fn accepts_platform_with_optional_variant() {
        assert!(validate_platform("linux/amd64").is_ok());
        assert!(validate_platform("linux/arm64/v8").is_ok());
    }

    #[test]
    fn rejects_platform_without_architecture() {
        assert!(validate_platform("linux").is_err());
        assert!(validate_platform("linux/").is_err());
        assert!(validate_platform("linux/arm 64").is_err());
    }
}
