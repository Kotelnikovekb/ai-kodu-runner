use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(
    name = "ai-codu-runer",
    version,
    about = "Isolated autonomous AI job runner"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    Doctor,
    Run {
        #[arg(long)]
        job: PathBuf,
        #[arg(long)]
        config: Option<PathBuf>,
    },
    Daemon {
        #[arg(long)]
        config: PathBuf,
    },
    Cleanup {
        #[arg(long)]
        config: PathBuf,
    },
    Version,
}
