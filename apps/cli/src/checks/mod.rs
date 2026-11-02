pub(crate) mod database;
pub(crate) mod discovery;
mod evidence;
mod model;
mod price_outlier;
mod state;

pub use state::Delivery;

#[derive(Debug)]
pub struct PendingChecks {
    pub response: ChecksResponse,
    pub delivery: Delivery,
}

impl PendingChecks {
    pub async fn write_json<W: tokio::io::AsyncWrite + Unpin>(
        self,
        writer: &mut W,
    ) -> Result<(), String> {
        use tokio::io::AsyncWriteExt;

        let result = async {
            let mut output = serde_json::to_vec(&self.response).map_err(std::io::Error::other)?;
            output.push(b'\n');
            writer.write_all(&output).await?;
            writer.flush().await
        }
        .await;
        if let Err(error) = result {
            let _ = self.delivery.finish(false).await;
            return Err(format!("could not write JSON output: {error}"));
        }
        self.delivery.finish(true).await
    }
}

pub use model::{CheckAssessment, ChecksResponse, EvidenceRecord, SessionIdentity};

pub(crate) async fn lookup(
    session_id: String,
    config: crate::config::Config,
) -> Result<PendingChecks, String> {
    tokio::task::spawn_blocking(move || {
        let state_path = config.state_path()?;
        let paths = discovery::candidate_database_paths(&config)?;
        let mut response = database::read_checks_from_paths(&session_id, &paths, &config)?;
        let delivery = state::prepare_delivery(&mut response, &state_path)?;
        Ok(PendingChecks { response, delivery })
    })
    .await
    .map_err(|error| format!("Antiburn database lookup task failed: {error}"))?
}
