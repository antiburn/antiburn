use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

use crate::config::MAX_SEARCH_LIMIT;

#[derive(Debug, Parser)]
#[command(
    name = "antiburn",
    bin_name = "antiburn",
    version,
    about = "Search and read Antiburn sessions and check updates"
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
    /// Report the CLI version and check-delivery protocols without opening a profile.
    Capabilities {
        #[arg(long, required = true)]
        json: bool,
    },
    /// Return new or changed findings for an exact local session ID as JSON.
    Checks {
        session_id: String,
        #[arg(long)]
        delivery_v1: bool,
        #[arg(long, requires = "delivery_v1")]
        consumer: Option<String>,
        #[arg(long, requires = "consumer")]
        ack: Option<String>,
    },
    /// Search session IDs and titles. Use --json for structured results.
    Search(SearchArgs),
    /// Read exact session metadata or one page of a Cloud document.
    Get(GetArgs),
}

/// Search terms and optional result limit supplied on the command line.
#[derive(Debug, Args)]
pub struct SearchArgs {
    /// Literal substring to match, ignoring ASCII case.
    #[arg(value_parser = parse_search_query)]
    pub(crate) query: String,
    /// Maximum results (1 to 100 locally; 1 to 50 for Cloud or combined search).
    #[arg(long, value_parser = clap::value_parser!(u16).range(1..=i64::from(MAX_SEARCH_LIMIT)))]
    pub(crate) limit: Option<u16>,
    #[arg(
        long,
        value_enum,
        help = if cfg!(all(feature = "cloud", unix)) {
            "Choose a source. The default includes Cloud when a device is connected."
        } else {
            "Choose a source. This build searches local sessions only."
        },
    )]
    pub(crate) source: Option<SearchMode>,
    /// Print structured JSON instead of a text summary.
    #[arg(long)]
    pub(crate) json: bool,
}

/// Sources selected for a search.
#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub(crate) enum SearchMode {
    /// Search only the local session index.
    Local,
    /// Search only the configured Cloud workspace.
    #[cfg(feature = "cloud")]
    #[value(hide = !cfg!(unix))]
    Cloud,
    /// Search both sources and merge only confirmed matches.
    #[cfg(feature = "cloud")]
    #[value(hide = !cfg!(unix))]
    All,
}

/// Read the exact identity from search, not a partial ID or a title.
#[derive(Debug, Args)]
pub struct GetArgs {
    /// The local session ID or the Cloud UUID returned by search.
    pub(crate) session_id: String,
    /// Select local metadata or an explicit Cloud document.
    #[arg(long, value_enum, default_value_t = GetSource::Local)]
    pub(crate) source: GetSource,
    /// Narrow a local ID to the agent returned by search.
    #[arg(long)]
    pub(crate) agent: Option<String>,
    /// Narrow a local ID to the environment_key returned by search.
    #[arg(long)]
    pub(crate) environment: Option<String>,
    /// Continue with the next cursor from the previous page.
    #[cfg(feature = "cloud")]
    #[arg(long, hide = !cfg!(unix))]
    pub(crate) after: Option<String>,
    /// Print structured JSON instead of a text summary.
    #[arg(long)]
    pub(crate) json: bool,
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub(crate) enum GetSource {
    /// Read local metadata, not the transcript.
    Local,
    /// Read one page of the Cloud document. Never substitute local data.
    #[cfg(feature = "cloud")]
    #[value(hide = !cfg!(unix))]
    Cloud,
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
