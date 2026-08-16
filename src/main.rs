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
mod cli;
mod config;
mod control;
mod executor;
mod janitor;
mod job;
mod journal;
mod telemetry;

use anyhow::Result;
use clap::Parser;
use cli::{Cli, Command};
use config::RunnerConfig;
use tracing::info;

#[tokio::main]
async fn main() -> Result<()> {
    telemetry::init();
    let cli = Cli::parse();
    match cli.command {
        Command::Version => println!("ai-codu-runer {}", env!("CARGO_PKG_VERSION")),
        Command::Doctor => executor::doctor().await?,
        Command::Run { job, config } => {
            let config = config.map_or_else(
                || Ok(RunnerConfig::default_local()),
                |path| RunnerConfig::load(&path),
            )?;
            let mut spec = job::JobSpec::from_path(&job)?;
            if spec.attempt == 0 {
                let journal = journal::Journal::open(&config.work_dir.join("runner.db"))?;
                spec.attempt = journal.next_attempt(&spec.id)?;
                info!(job_id=%spec.id, attempt=spec.attempt, "allocated local job attempt");
            }
            let factory = executor::factory(config, None)?;
            let result = factory.select_for(&spec)?.run(spec, None).await?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        Command::Cleanup { config } => {
            let config = RunnerConfig::load(&config)?;
            let factory = executor::factory(config, None)?;
            janitor::cleanup(factory.get("docker")?.as_ref()).await?;
        }
        Command::Daemon { config } => {
            let config = RunnerConfig::load(&config)?;
            info!(runner_id = %config.runner_id(), "daemon starting");
            control::run_daemon(config).await?;
        }
    }
    Ok(())
}
