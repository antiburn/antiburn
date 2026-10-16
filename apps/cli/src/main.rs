use std::process::ExitCode;

use clap::Parser;
use tokio::io::AsyncWriteExt;

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    if let Err(error) = antiburn_cli::init_tracing() {
        report_error(&format!("antiburn: file tracing unavailable: {error}")).await;
    }

    let cli = antiburn_cli::cli::Cli::parse();
    let response = match antiburn_cli::run(cli).await {
        Ok(response) => response,
        Err(error) => {
            tracing::error!(event = "cli_command_failed", error = %error);
            report_error(&format!("antiburn: {error}")).await;
            return ExitCode::from(error.exit_code());
        }
    };

    let mut output = match serde_json::to_vec(&response).map_err(std::io::Error::other) {
        Ok(output) => output,
        Err(error) => {
            tracing::error!(event = "json_serialization_failed", error = %error);
            report_error(&format!("antiburn: could not write JSON output: {error}")).await;
            return ExitCode::FAILURE;
        }
    };
    output.push(b'\n');

    let mut stdout = tokio::io::stdout();
    let write_result = async {
        stdout.write_all(&output).await?;
        stdout.flush().await
    }
    .await;
    if let Err(error) = write_result {
        tracing::error!(event = "stdout_write_failed", error = %error);
        report_error(&format!("antiburn: could not write JSON output: {error}")).await;
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
