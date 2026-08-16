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

use runner_core::executor::ExecutorFactory;
use runner_core::mock::MockExecutor;
use runner_protocol::{ExecutionRequirements, IsolationLevel, JobSpec};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn community_executor_conformance_covers_doctor_run_cancel_and_cleanup() {
    let mut factory = ExecutorFactory::default();
    factory
        .register("docker", Arc::new(MockExecutor))
        .expect("mock registration");
    let spec = JobSpec::from_json(include_str!(
        "../../runner-protocol/tests/fixtures/job-spec-v1alpha1.json"
    ))
    .unwrap();
    let executor = factory.select_for(&spec).unwrap();

    let report = executor.doctor().await.unwrap();
    assert!(report.healthy);
    assert!(report.capabilities.supports("cancellation"));

    let result = executor.run(spec.clone(), None).await.unwrap();
    assert_eq!(result.status, "completed");
    assert_eq!(result.sandbox.executor, "mock");

    let cancellation = CancellationToken::new();
    cancellation.cancel();
    let cancelled = executor.run(spec, Some(cancellation)).await.unwrap();
    assert_eq!(cancelled.status, "cancelled");
    assert!(cancelled.exit_code.is_none());

    executor.cleanup().await.unwrap();
}

#[test]
fn community_admission_rejects_stronger_isolation_and_unknown_capabilities() {
    let mut factory = ExecutorFactory::default();
    factory
        .register("docker", Arc::new(MockExecutor))
        .expect("mock registration");
    let mut spec = JobSpec::from_json(include_str!(
        "../../runner-protocol/tests/fixtures/job-spec-v1alpha1.json"
    ))
    .unwrap();

    spec.execution = Some(ExecutionRequirements {
        isolation: IsolationLevel::Sandboxed,
        capabilities: Vec::new(),
    });
    assert!(factory.select_for(&spec).is_err());

    spec.execution = Some(ExecutionRequirements {
        isolation: IsolationLevel::Container,
        capabilities: vec!["unknown_capability".into()],
    });
    assert!(factory.select_for(&spec).is_err());
}
