use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name = "antiburn",
    version,
    about = "Read Antiburn check updates for an agent session"
)]
pub struct Cli {
    #[arg(
        long,
        global = true,
        value_name = "PATH",
        help = "Path to a JSON config file"
    )]
    pub config: Option<PathBuf>,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    Checks {
        session_id: String,
        #[arg(long)]
        delivery_v1: bool,
        #[arg(long, requires = "delivery_v1")]
        consumer: Option<String>,
        #[arg(long, requires = "consumer")]
        ack: Option<String>,
    },
}
