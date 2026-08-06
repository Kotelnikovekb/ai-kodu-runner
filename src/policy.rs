use crate::{
    config::RunnerConfig,
    job::{JobSpec, Resources, WorkspaceSpec},
};
use anyhow::{Result, bail};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
pub fn clamp(r: &Resources, c: &RunnerConfig) -> Resources {
    Resources {
        cpu: r.cpu.min(c.limits.max_cpu).max(0.01),
        memory_mb: r.memory_mb.min(c.limits.max_memory_mb).max(16),
        pids: r.pids.min(c.limits.max_pids).max(1),
        timeout_seconds: r.timeout_seconds.min(c.limits.max_timeout_seconds).max(1),
    }
}
pub fn validate(spec: &JobSpec, c: &RunnerConfig, daemon: bool) -> Result<Resources> {
    spec.validate(daemon)?;
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
}
