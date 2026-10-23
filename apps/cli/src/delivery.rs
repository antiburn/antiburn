mod state;

use crate::{
    checks::{SessionIdentity, database, discovery},
    config::Config,
};
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct DeliveryCheck {
    pub id: String,
    pub check_id: String,
    pub name: String,
    pub message: Option<String>,
    pub status: Option<String>,
    pub assessed_at_epoch: Option<i64>,
    pub checking: bool,
    pub eligibility: String,
}

#[derive(Debug, Serialize)]
pub struct Update {
    pub id: String,
    pub kind: &'static str,
    pub name: String,
    pub message: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct DeliveryResponse {
    pub protocol_version: u32,
    pub session_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub identity: Option<SessionIdentity>,
    pub freshness_evaluated: bool,
    pub session_scope: String,
    pub state: String,
    pub snapshot: Vec<DeliveryCheck>,
    pub updates: Vec<Update>,
    pub receipt: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub acknowledged: Option<bool>,
}

pub async fn lookup(
    session_id: String,
    consumer: String,
    ack: Option<String>,
    config: Config,
) -> Result<DeliveryResponse, String> {
    if consumer.is_empty()
        || consumer.len() > 128
        || !consumer
            .bytes()
            .all(|v| v.is_ascii_alphanumeric() || b":-_.".contains(&v))
    {
        return Err("consumer must contain 1 to 128 ASCII letters, digits, colons, dots, hyphens, or underscores".into());
    }
    if ack.is_some() && consumer.ends_with(":status") {
        return Err("status snapshots do not support acknowledgement".into());
    }
    if session_id.len() > 512
        || ack
            .as_ref()
            .is_some_and(|v| v.len() != 64 || !v.bytes().all(|b| b.is_ascii_hexdigit()))
    {
        return Err("invalid delivery session ID or receipt".into());
    }
    tokio::task::spawn_blocking(move || read(session_id, consumer, ack, config))
        .await
        .map_err(|error| format!("delivery task failed: {error}"))?
}

fn read(
    session_id: String,
    consumer: String,
    ack: Option<String>,
    config: Config,
) -> Result<DeliveryResponse, String> {
    let mut response = DeliveryResponse {
        protocol_version: 1,
        session_id: session_id.clone(),
        identity: None,
        freshness_evaluated: false,
        session_scope: "unsupported".into(),
        state: "waiting".into(),
        snapshot: vec![],
        updates: vec![],
        receipt: None,
        acknowledged: None,
    };
    let paths = match discovery::candidate_database_paths(&config) {
        Ok(paths) => paths,
        Err(_) => {
            response.state = "missing-database".into();
            return Ok(response);
        }
    };
    let mut selected = None;
    for path in paths {
        let connection = database::open_read_only(
            &path,
            config
                .busy_timeout
                .min(std::time::Duration::from_millis(250)),
        )?;
        let identities = database::session_identities(&connection, &session_id)?;
        if identities.len() > 1 || (!identities.is_empty() && selected.is_some()) {
            return Err("delivery session ID matches multiple identities".into());
        }
        if let Some(identity) = identities.into_iter().next() {
            selected = Some((connection, identity));
        }
    }
    let Some((mut connection, identity)) = selected else {
        return Err("delivery session ID was not found".into());
    };
    let main = connection.query_row(
        "SELECT environment_key = 'native' AND wsl_distro IS NULL AND cwd IS NOT NULL AND cwd <> ''
         AND (s.agent <> 'cursor' OR (s.source_kind = 'inline' AND s.source_label GLOB 'cursor-desktop:*'))
         AND NOT EXISTS(SELECT 1 FROM session_relation r
           WHERE r.environment_key = s.environment_key AND r.agent = s.agent
           AND ((r.session_id = s.session_id AND r.kind = 'forkParent')
             OR (r.related_id = s.session_id AND r.kind = 'subagent')))
         FROM session s WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3",
        rusqlite::params![identity.environment_key, identity.agent, identity.session_id], |row| row.get::<_,bool>(0)
    ).map_err(|error| format!("could not verify main session: {error}"))?;
    response.session_scope = if main { "main" } else { "unsupported" }.into();
    let state_path = state::path(&config.state_path()?);
    let state_key = state::key(&identity, &consumer)?;
    if let Some(receipt) = ack {
        state::acknowledge(&state_path, &state_key, &receipt)?;
        response.acknowledged = Some(true);
        response.identity = Some(identity);
        response.state = "acknowledged".into();
        return Ok(response);
    }
    let transaction = connection
        .transaction()
        .map_err(|error| format!("could not start delivery read: {error}"))?;
    let compatible: bool = transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'burn_check_runtime_definition')",
        [], |row| row.get(0),
    ).map_err(|error| format!("could not inspect delivery schema: {error}"))?;
    if !compatible {
        response.state = "incompatible".into();
        response.identity = Some(identity);
        return Ok(response);
    }
    match antiburn_checks_store::runtime_snapshot_ready(&transaction) {
        Ok(true) => {}
        Ok(false) => {
            response.state = "waiting".into();
            response.identity = Some(identity);
            return Ok(response);
        }
        Err(_) => {
            response.state = "incompatible".into();
            response.identity = Some(identity);
            return Ok(response);
        }
    }
    let checks = antiburn_checks_store::read_session_checks(
        &transaction,
        &identity.environment_key,
        &identity.agent,
        &identity.session_id,
    );
    let checks = match checks {
        Ok(checks) => checks,
        Err(_) => {
            response.state = "unavailable".into();
            response.identity = Some(identity);
            return Ok(response);
        }
    };
    let eligibility = checks
        .iter()
        .map(|check| check.eligibility)
        .collect::<Vec<_>>();
    for check in checks {
        response.snapshot.push(DeliveryCheck {
            id: check.id,
            check_id: check.check_id,
            name: check.name,
            message: check.message,
            status: check.status.map(|status| match status {
                antiburn_checks_store::CheckStatus::Failure => "failure".into(),
                antiburn_checks_store::CheckStatus::Success => "success".into(),
            }),
            assessed_at_epoch: check.assessed_at_epoch,
            checking: check.checking,
            eligibility: format!("{:?}", check.eligibility).to_lowercase(),
        });
    }
    transaction
        .commit()
        .map_err(|error| format!("could not finish delivery read: {error}"))?;
    response.state = if !main
        || identity.environment_key != "native"
        || !matches!(
            identity.agent.as_str(),
            "claude-code" | "codex" | "pi" | "opencode" | "cursor"
        ) {
        "unsupported"
    } else if response.snapshot.is_empty()
        || eligibility
            .iter()
            .all(|e| *e == antiburn_checks_store::Eligibility::Unconfigured)
    {
        "unconfigured"
    } else if eligibility
        .iter()
        .all(|e| *e == antiburn_checks_store::Eligibility::Unsupported)
    {
        "unsupported"
    } else if eligibility
        .iter()
        .all(|e| *e == antiburn_checks_store::Eligibility::Paused)
    {
        "disabled"
    } else if eligibility.contains(&antiburn_checks_store::Eligibility::Ready) {
        "ready"
    } else if eligibility.contains(&antiburn_checks_store::Eligibility::Waiting) {
        "waiting"
    } else {
        "unavailable"
    }
    .into();
    if !consumer.ends_with(":status") {
        let (updates, receipt) = state::stage(&state_path, &state_key, &response.snapshot)?;
        response.updates = updates;
        response.receipt = receipt;
    }
    response.identity = Some(identity);
    Ok(response)
}
