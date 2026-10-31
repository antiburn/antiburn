use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name = "antiburn",
    bin_name = "antiburn",
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
    Capabilities {
        #[arg(long, required = true)]
        json: bool,
    },
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn help_uses_the_public_name_for_platform_and_runtime_executable_names() {
        for executable in ["antiburn", "antiburn.exe", "antiburn-cli.exe"] {
            let error = Cli::try_parse_from([executable, "--help"])
                .expect_err("Help returns Clap's display result");
            assert_eq!(error.kind(), clap::error::ErrorKind::DisplayHelp);
            let help = error.to_string();
            assert!(help.contains("Usage: antiburn "), "{help}");
            assert!(!help.contains("Usage: antiburn.exe"), "{help}");
            assert!(!help.contains("Usage: antiburn-cli"), "{help}");
        }
    }
}
