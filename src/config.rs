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
use crate::job::CommandSpec;
use anyhow::{Context, Result};
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
}

fn default_max_artifact_files() -> usize {
    10_000
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
        toml::from_str(
            &fs::read_to_string(path).with_context(|| format!("read config {}", path.display()))?,
        )
        .context("parse runner.toml")
    }
    pub fn default_local() -> Self {
        Self {
            server_url: None,
            runner_token: None,
            name: Some("local".into()),
            work_dir: env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            concurrency: Some(1),
            docker: DockerConfig {
                host: Some("auto".into()),
                pull_policy: Some("if-not-present".into()),
            },
            limits: Limits {
                max_cpu: 4.0,
                max_memory_mb: 8192,
                max_pids: 512,
                max_timeout_seconds: 3600,
                max_log_bytes: 50 * 1024 * 1024,
                max_artifact_bytes: 500 * 1024 * 1024,
                max_artifact_files: default_max_artifact_files(),
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
        self.name
            .clone()
            .unwrap_or_else(|| Uuid::new_v4().to_string())
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
