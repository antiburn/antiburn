use std::process::ExitCode;

use clap::Parser;
use tokio::io::AsyncWriteExt;

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let cli = antiburn_cli::cli::Cli::parse();
    let response = match antiburn_cli::run_with_tracing(cli).await {
        Ok(response) => response,
        Err(error) => {
            tracing::error!(event = "cli_command_failed", error = %error);
            report_error(&format!("antiburn: {error}")).await;
            return ExitCode::from(error.exit_code());
        }
    };

    if let Err(error) = response.write_json(&mut tokio::io::stdout()).await {
        tracing::error!(event = "cli_delivery_failed", error = %error);
        report_error(&format!("antiburn: {error}")).await;
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

async fn report_error(message: &str) {
    let mut stderr = tokio::io::stderr();
    let _ = stderr.write_all(message.as_bytes()).await;
    let _ = stderr.write_all(b"\n").await;
    let _ = stderr.flush().await;
}
