mod database;
mod discovery;
mod evidence;
mod model;
mod state;

pub use model::{CheckAssessment, ChecksResponse, EvidenceRecord, SessionIdentity};

pub(crate) async fn lookup(
    session_id: String,
    config: crate::config::Config,
) -> Result<ChecksResponse, String> {
    tokio::task::spawn_blocking(move || {
        let state_path = config.state_path()?;
        let paths = discovery::candidate_database_paths(&config)?;
        let mut response =
            database::read_checks_from_paths(&session_id, &paths, config.busy_timeout)?;
        state::return_new_findings(&mut response, &state_path)?;
        Ok(response)
    })
    .await
    .map_err(|error| format!("Antiburn database lookup task failed: {error}"))?
}
