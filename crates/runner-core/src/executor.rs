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

use anyhow::Result;
use async_trait::async_trait;
use runner_protocol::{ExecutionRequirements, IsolationLevel, JobResult, JobSpec};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutorCapabilities {
    #[serde(default)]
    pub isolation: IsolationLevel,
    #[serde(default)]
    pub capabilities: BTreeSet<String>,
}

impl Default for ExecutorCapabilities {
    fn default() -> Self {
        Self {
            isolation: IsolationLevel::Container,
            capabilities: BTreeSet::new(),
        }
    }
}

impl ExecutorCapabilities {
    pub fn new(names: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            isolation: IsolationLevel::Container,
            capabilities: names.into_iter().map(Into::into).collect(),
        }
    }

    pub fn supports(&self, name: &str) -> bool {
        self.capabilities.contains(name)
    }

    pub fn satisfies(&self, requirements: &ExecutionRequirements) -> Result<()> {
        if requirements.isolation != IsolationLevel::Container
            && requirements.isolation != self.isolation
        {
            anyhow::bail!(
                "executor does not support required isolation: {:?}",
                requirements.isolation
            )
        }
        for capability in &requirements.capabilities {
            if !self.supports(capability) {
                anyhow::bail!("executor does not support capability: {capability}")
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DoctorCheck {
    pub name: String,
    pub healthy: bool,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DoctorReport {
    pub executor: String,
    pub healthy: bool,
    pub capabilities: ExecutorCapabilities,
    pub checks: Vec<DoctorCheck>,
}

#[async_trait]
pub trait Executor: Send + Sync {
    fn capabilities(&self) -> ExecutorCapabilities;

    async fn doctor(&self) -> Result<DoctorReport>;

    async fn run(&self, spec: JobSpec, cancel: Option<CancellationToken>) -> Result<JobResult>;

    async fn cleanup(&self) -> Result<()>;
}

#[derive(Default)]
pub struct ExecutorFactory {
    executors: BTreeMap<String, Arc<dyn Executor>>,
}

impl ExecutorFactory {
    pub fn register(&mut self, name: impl Into<String>, executor: Arc<dyn Executor>) -> Result<()> {
        let name = name.into();
        if name.trim().is_empty() {
            anyhow::bail!("executor name must not be empty")
        }
        if self.executors.contains_key(&name) {
            anyhow::bail!("executor already registered: {name}")
        }
        self.executors.insert(name, executor);
        Ok(())
    }

    pub fn get(&self, name: &str) -> Result<Arc<dyn Executor>> {
        self.executors
            .get(name)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("executor is not registered: {name}"))
    }

    pub fn select_for(&self, spec: &JobSpec) -> Result<Arc<dyn Executor>> {
        let executor = self.get(&spec.executor)?;
        let requirements = spec.execution.clone().unwrap_or(ExecutionRequirements {
            isolation: IsolationLevel::Container,
            capabilities: Vec::new(),
        });
        executor
            .capabilities()
            .satisfies(&requirements)
            .map_err(|error| anyhow::anyhow!("executor admission failed: {error}"))?;
        Ok(executor)
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.executors.keys().map(String::as_str)
    }
}

#[cfg(test)]
mod tests {
    use super::{Executor, ExecutorCapabilities, ExecutorFactory};
    use crate::mock::MockExecutor;
    use runner_protocol::{ExecutionRequirements, IsolationLevel};
    use std::sync::Arc;

    #[test]
    fn capabilities_are_extensible_and_deterministic() {
        let capabilities = ExecutorCapabilities::new(["network", "artifacts", "network"]);

        assert!(capabilities.supports("network"));
        assert_eq!(capabilities.capabilities.len(), 2);
    }

    #[test]
    fn capabilities_reject_unsupported_isolation_and_names() {
        let capabilities = ExecutorCapabilities::new(["artifacts"]);

        assert!(
            capabilities
                .satisfies(&ExecutionRequirements {
                    isolation: IsolationLevel::Container,
                    capabilities: vec!["artifacts".into()],
                })
                .is_ok()
        );
        assert!(
            capabilities
                .satisfies(&ExecutionRequirements {
                    isolation: IsolationLevel::Sandboxed,
                    capabilities: Vec::new(),
                })
                .is_err()
        );
        assert!(
            capabilities
                .satisfies(&ExecutionRequirements {
                    isolation: IsolationLevel::Container,
                    capabilities: vec!["unknown".into()],
                })
                .is_err()
        );
    }

    #[tokio::test]
    async fn factory_rejects_duplicate_and_unknown_executors() {
        let executor = Arc::new(MockExecutor);
        let mut factory = ExecutorFactory::default();

        factory.register("mock", executor.clone()).unwrap();
        assert!(factory.register("mock", executor).is_err());
        assert!(factory.get("missing").is_err());
        assert_eq!(
            factory.get("mock").unwrap().capabilities(),
            MockExecutor.capabilities()
        );
    }
}
