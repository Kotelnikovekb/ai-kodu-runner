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
use crate::config::RunnerConfig;
use anyhow::{Result, bail};
use runner_protocol::{JobSpec, Resources, WorkspaceSpec};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValidationContext {
    Local,
    Daemon,
}

impl ValidationContext {
    fn requires_digest(self) -> bool {
        matches!(self, Self::Daemon)
    }
}
pub fn clamp(r: &Resources, c: &RunnerConfig) -> Resources {
    Resources {
        cpu: r.cpu.min(c.limits.max_cpu).max(0.01),
        memory_mb: r.memory_mb.min(c.limits.max_memory_mb).max(16),
        pids: r.pids.min(c.limits.max_pids).max(1),
        timeout_seconds: r.timeout_seconds.min(c.limits.max_timeout_seconds).max(1),
    }
}
pub fn validate(spec: &JobSpec, c: &RunnerConfig, daemon: bool) -> Result<Resources> {
    validate_with_context(
        spec,
        c,
        if daemon {
            ValidationContext::Daemon
        } else {
            ValidationContext::Local
        },
    )
}

pub fn validate_with_context(
    spec: &JobSpec,
    c: &RunnerConfig,
    context: ValidationContext,
) -> Result<Resources> {
    spec.validate(context.requires_digest())?;
    if !c.security.allow_network && spec.network.mode == "bridge" {
        bail!("network disabled by local policy")
    }
    if spec
        .environment_from_runner
        .iter()
        .any(|x| !c.security.allowed_environment.contains(x))
    {
        bail!("job requests an environment variable not allowed by policy")
    }
    let mut names = HashSet::new();
    for name in &spec.environment_from_runner {
        if !names.insert(name) {
            bail!("duplicate secret name: {name}")
        }
    }
    for secret in &spec.secrets {
        if !c.security.allowed_environment.contains(&secret.name) {
            bail!("secret is not allowed by local policy: {}", secret.name)
        }
        if secret.secret_ref.is_some() {
            bail!("secret_ref is not resolvable by the Community executor")
        }
        if !names.insert(&secret.name) {
            bail!("duplicate secret name: {}", secret.name)
        }
    }
    let r = clamp(&spec.resources, c);
    let root = c
        .work_dir
        .canonicalize()
        .unwrap_or_else(|_| c.work_dir.clone());
    if let WorkspaceSpec::Local { path } = &spec.workspace {
        let p = PathBuf::from(path);
        let resolved = if p.is_absolute() { p } else { root.join(p) };
        if !is_within(&resolved, &root) {
            bail!("local workspace must be inside configured work_dir")
        }
    }
    Ok(r)
}
pub fn environment_for_job(spec: &JobSpec, c: &RunnerConfig) -> Result<Vec<String>> {
    let mut env = Vec::new();
    for key in &spec.environment_from_runner {
        let value =
            std::env::var(key).map_err(|_| anyhow::anyhow!("missing environment secret: {key}"))?;
        env.push(format!("{key}={value}"));
    }
    for secret in &spec.secrets {
        if !c.security.allowed_environment.contains(&secret.name) {
            bail!("secret is not allowed by local policy: {}", secret.name)
        }
        if secret.secret_ref.is_some() {
            bail!("secret_ref is not resolvable by the Community executor")
        }
        env.push(format!("{}={}", secret.name, secret.value));
    }
    Ok(env)
}
pub fn is_within(path: &Path, root: &Path) -> bool {
    path.canonicalize()
        .map(|p| p.starts_with(root))
        .unwrap_or(false)
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::RunnerConfig;
    use runner_protocol::SecretSpec;
    #[test]
    fn clamps() {
        let c = RunnerConfig::default_local();
        let r = clamp(
            &Resources {
                cpu: 99.,
                memory_mb: 99999,
                pids: 9999,
                timeout_seconds: 9999,
            },
            &c,
        );
        assert_eq!(r.cpu, 4.);
        assert_eq!(r.memory_mb, 8192);
    }

    #[test]
    fn community_fails_closed_on_secret_ref() {
        let mut config = RunnerConfig::default_local();
        config.security.allowed_environment = vec!["MODEL_KEY".into()];
        let mut spec = JobSpec::from_json(
            r#"{
                "api_version":"omniroute.dev/v1alpha1",
                "id":"secret-ref",
                "attempt":1,
                "executor":"docker",
                "image":"example/runner:latest",
                "command":["true"],
                "working_directory":"/workspace",
                "workspace":{"kind":"local","path":"."},
                "resources":{"cpu":1.0,"memory_mb":256,"pids":64,"timeout_seconds":60},
                "network":{"mode":"none"},
                "secrets":[]
            }"#,
        )
        .unwrap();
        spec.secrets = vec![SecretSpec {
            name: "MODEL_KEY".into(),
            value: String::new(),
            secret_ref: Some("vault://model-key".into()),
        }];

        let error = environment_for_job(&spec, &config).unwrap_err();
        assert!(error.to_string().contains("not resolvable"));
    }

    #[test]
    fn daemon_context_requires_digest_pinned_images() {
        let config = RunnerConfig::default_local();
        let spec = JobSpec::from_json(
            r#"{
                "api_version":"omniroute.dev/v1alpha1",
                "id":"mutable-image",
                "attempt":1,
                "executor":"docker",
                "image":"example/runner:latest",
                "command":["true"],
                "working_directory":"/workspace",
                "workspace":{"kind":"local","path":"."},
                "resources":{"cpu":1.0,"memory_mb":256,"pids":64,"timeout_seconds":60},
                "network":{"mode":"none"},
                "secrets":[]
            }"#,
        )
        .unwrap();

        assert!(validate_with_context(&spec, &config, ValidationContext::Local).is_ok());
        let error = validate_with_context(&spec, &config, ValidationContext::Daemon).unwrap_err();
        assert!(error.to_string().contains("image digest"));
    }
}
