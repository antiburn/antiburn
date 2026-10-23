pub mod checks;
pub mod cli;
mod config;
mod delivery;
mod logging;

use crate::cli::Command;

#[derive(Debug)]
pub struct CliError {
    message: String,
    exit_code: u8,
}

impl CliError {
    fn usage(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            exit_code: 2,
        }
    }

    fn runtime(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            exit_code: 1,
        }
    }

    pub const fn exit_code(&self) -> u8 {
        self.exit_code
    }
}

impl std::fmt::Display for CliError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for CliError {}

pub fn init_tracing() -> std::io::Result<()> {
    logging::init()
}

#[derive(serde::Serialize)]
#[serde(untagged)]
pub enum Response {
    Checks(checks::ChecksResponse),
    Delivery(delivery::DeliveryResponse),
}

pub async fn run(cli: cli::Cli) -> Result<Response, CliError> {
    let cli::Cli { config, command } = cli;
    let Command::Checks {
        session_id,
        delivery_v1,
        consumer,
        ack,
    } = command;
    validate_id(&session_id).map_err(CliError::usage)?;
    let config = config::Config::load(config)
        .await
        .map_err(CliError::runtime)?;

    if delivery_v1 {
        let consumer =
            consumer.ok_or_else(|| CliError::usage("--delivery-v1 requires --consumer"))?;
        return delivery::lookup(session_id, consumer, ack, config)
            .await
            .map(Response::Delivery)
            .map_err(CliError::runtime);
    }
    let response = checks::lookup(session_id, config)
        .await
        .map_err(CliError::runtime)?;
    tracing::info!(
        event = "checks_lookup_completed",
        session_id = %response.session_id,
        check_count = response.checks.len(),
        evidence_status = %response.evidence.status,
    );

    Ok(Response::Checks(response))
}

fn validate_id(value: &str) -> Result<(), String> {
    if value.trim().is_empty() || value.chars().any(char::is_control) {
        return Err("session IDs cannot be empty or contain control characters".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use crate::{cli::Cli, validate_id};

    #[test]
    fn checks_command_parses_one_session_id() {
        let cli = Cli::try_parse_from(["antiburn", "checks", "session-123"])
            .expect("checks command should parse");
        assert!(
            matches!(cli.command, super::Command::Checks { session_id, .. } if session_id == "session-123")
        );
    }

    #[test]
    fn config_path_can_override_the_default_json_config() {
        let cli = Cli::try_parse_from([
            "antiburn",
            "--config",
            "/tmp/antiburn-cli.json",
            "checks",
            "session-123",
        ])
        .expect("config option should parse");
        assert_eq!(
            cli.config.as_deref(),
            Some(std::path::Path::new("/tmp/antiburn-cli.json"))
        );
    }

    #[test]
    fn session_id_validation_rejects_only_empty_and_control_values() {
        assert!(validate_id("valid-session_1:2.3/4").is_ok());
        assert!(validate_id("").is_err());
        assert!(validate_id("   ").is_err());
        assert!(validate_id("session\tvalue").is_err());
        assert!(validate_id("session\nvalue").is_err());
    }
    #[test]
    fn delivery_flags_require_the_versioned_mode() {
        assert!(
            Cli::try_parse_from(["antiburn", "checks", "session", "--consumer", "pi:context"])
                .is_err()
        );
        assert!(
            Cli::try_parse_from([
                "antiburn",
                "checks",
                "session",
                "--delivery-v1",
                "--ack",
                "receipt"
            ])
            .is_err()
        );
        let cli = Cli::try_parse_from([
            "antiburn",
            "checks",
            "session",
            "--delivery-v1",
            "--consumer",
            "pi:context",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            super::Command::Checks {
                delivery_v1: true,
                consumer: Some(_),
                ..
            }
        ));
    }
}
