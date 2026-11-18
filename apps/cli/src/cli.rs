use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

use crate::config::MAX_SEARCH_LIMIT;

#[derive(Debug, Parser)]
#[command(
    name = "antiburn",
    bin_name = "antiburn",
    version,
    about = "Read Antiburn check updates and search local sessions"
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
    /// Search local Antiburn session IDs and titles. Return JSON only.
    Search(SearchArgs),
}

/// Search terms and optional result limit supplied on the command line.
#[derive(Debug, Args)]
pub struct SearchArgs {
    /// Literal substring to match, ignoring ASCII case.
    #[arg(value_parser = parse_search_query)]
    pub(crate) query: String,
    /// Maximum number of results to return (1 to 100).
    #[arg(long, value_parser = clap::value_parser!(u16).range(1..=i64::from(MAX_SEARCH_LIMIT)))]
    pub(crate) limit: Option<u16>,
}

fn parse_search_query(value: &str) -> Result<String, String> {
    if value.chars().any(char::is_control) {
        return Err("search query cannot contain control characters".into());
    }
    let query = value.trim();
    if query.is_empty() || query.len() > 512 {
        return Err("search query must contain 1 to 512 bytes after trimming".into());
    }
    Ok(query.to_owned())
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
