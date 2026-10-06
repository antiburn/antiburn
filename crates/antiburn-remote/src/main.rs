use std::io::{Read, Write};

use antiburn_remote::{
    Hello, MAX_REQUEST_BYTES, MAX_RESPONSE_BYTES, Request, SESSION_REJECTED_EXIT_CODE, collector,
};

#[tokio::main(flavor = "current_thread")]
async fn main() {
    if let Err(failure) = run().await {
        let (error, exit_code) = match failure {
            RunFailure::Fatal(error) => (error, 1),
            RunFailure::SessionRejected(error) => (error, SESSION_REJECTED_EXIT_CODE),
        };
        eprintln!("antiburn-remote: {error}");
        std::process::exit(exit_code);
    }
}

enum RunFailure {
    Fatal(anyhow::Error),
    SessionRejected(anyhow::Error),
}

async fn run() -> Result<(), RunFailure> {
    run_validated().await.map_err(|error| {
        if error.downcast_ref::<ExportRejection>().is_some() {
            RunFailure::SessionRejected(error)
        } else {
            RunFailure::Fatal(error)
        }
    })
}

async fn run_validated() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.as_slice() == ["--version"] {
        println!("antiburn-remote {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    anyhow::ensure!(
        args.as_slice() == ["stdio"] || (args.len() == 2 && args[0] == "ssh"),
        "Usage: antiburn-remote stdio | ssh HOST | --version"
    );
    let mut bytes = Vec::new();
    std::io::stdin()
        .take(MAX_REQUEST_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    anyhow::ensure!(bytes.len() <= MAX_REQUEST_BYTES, "Request exceeds 8 KiB");
    let request: Request = serde_json::from_slice(&bytes)?;
    request.validate()?;

    if args[0] == "ssh" {
        anyhow::ensure!(
            !matches!(request, Request::Export { .. }),
            "Binary export requires the desktop sync client or stdio mode"
        );
        let response = antiburn_remote::transport::request(&args[1], &request).await?;
        std::io::stdout().write_all(&response)?;
        return Ok(());
    }

    if let Request::Export {
        agent,
        session_id,
        known,
        ..
    } = &request
    {
        return antiburn_remote::export::write_bundle(
            agent,
            session_id,
            known.as_deref(),
            &mut std::io::stdout(),
        )
        .await
        .map_err(|error| anyhow::Error::new(ExportRejection(error)));
    }
    let response = match request {
        Request::Export { .. } => unreachable!(),
        Request::Hello { .. } => serde_json::to_vec(&Hello::current())?,
        Request::List { .. } => serde_json::to_vec(&collector::list().await)?,
        Request::Analyze {
            agent, session_id, ..
        } => serde_json::to_vec(&collector::analyze(&agent, &session_id).await?)?,
    };
    anyhow::ensure!(
        response.len() as u64 <= MAX_RESPONSE_BYTES,
        "Response exceeds 8 MiB"
    );
    std::io::stdout().write_all(&response)?;
    Ok(())
}

#[derive(Debug)]
struct ExportRejection(anyhow::Error);

impl std::fmt::Display for ExportRejection {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(formatter)
    }
}

impl std::error::Error for ExportRejection {}
