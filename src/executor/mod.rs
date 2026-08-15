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
pub mod docker;
use crate::job::{JobResult, JobSpec};
use anyhow::Result;
use async_trait::async_trait;
use tokio_util::sync::CancellationToken;
#[async_trait]
pub trait Executor {
    async fn run(&self, spec: JobSpec, cancel: Option<CancellationToken>) -> Result<JobResult>;
}
