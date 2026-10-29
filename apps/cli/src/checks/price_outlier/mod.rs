mod cohort;
mod cost;

#[cfg(test)]
mod tests;

use rusqlite::Connection;
use serde_json::json;
use std::path::Path;

use crate::{
    checks::model::{CheckAssessment, CheckAssessmentSource, SessionIdentity},
    config::{Config, PriceOutlierConfig},
};

const CHECK_ID: &str = "outlier_priced_session";

struct SessionRow {
    identity: SessionIdentity,
    updated_at_epoch: Option<i64>,
    model_breakdown_json: Option<String>,
    pricing_breakdown_json: Option<String>,
}

pub(super) fn assessment(
    connection: &Connection,
    database_path: &Path,
    identity: &SessionIdentity,
    config: &Config,
) -> Option<CheckAssessment> {
    let target = match cohort::target(connection, identity) {
        Ok(Some(target)) => target,
        Ok(None) => return None,
        Err(_) => {
            tracing::warn!(event = "session_price_outlier_target_unavailable");
            return None;
        }
    };
    if target.model_breakdown_json.is_none() && target.pricing_breakdown_json.is_none() {
        return None;
    }
    let Some(target_epoch) = target.updated_at_epoch else {
        return Some(not_assessed("missing_target_timestamp"));
    };
    let pricing = match cost::load_pricing(database_path, config.pricing_snapshot_path.as_deref()) {
        Ok(Some(pricing)) => pricing,
        Ok(None) | Err(_) => {
            tracing::warn!(event = "session_price_outlier_pricing_unavailable");
            return Some(not_assessed("pricing_unavailable"));
        }
    };
    let Some(target_cost) = cost::session_cost(
        target.model_breakdown_json.as_deref(),
        target.pricing_breakdown_json.as_deref(),
        &pricing,
    ) else {
        return Some(not_assessed("target_cost_unavailable"));
    };
    let Some(max_age_seconds) = max_age_seconds(config.price_outlier) else {
        return Some(not_assessed("cohort_policy_unavailable"));
    };
    let neighbor_limit = config.price_outlier.max_sessions.saturating_sub(1);
    let neighbors = match cohort::neighbors(
        connection,
        identity,
        target_epoch,
        max_age_seconds,
        i64::try_from(neighbor_limit).ok()?,
    ) {
        Ok(neighbors) => neighbors,
        Err(_) => {
            tracing::warn!(event = "session_price_outlier_cohort_unavailable");
            return Some(not_assessed("cohort_unavailable"));
        }
    };
    let mut costs = vec![target_cost];
    costs.extend(neighbors.iter().filter_map(|neighbor| {
        if is_target(&neighbor.identity, identity) {
            return None;
        }
        cost::session_cost(
            neighbor.model_breakdown_json.as_deref(),
            neighbor.pricing_breakdown_json.as_deref(),
            &pricing,
        )
    }));
    let policy = config.price_outlier;
    if costs.len() < policy.minimum_priced_sessions {
        return Some(not_assessed_with_sample(
            "insufficient_priced_cohort",
            costs.len(),
        ));
    }
    let Some(threshold) = cost::outlier_threshold(&costs, policy) else {
        return Some(not_assessed_with_sample(
            "threshold_unavailable",
            costs.len(),
        ));
    };
    let is_finding = target_cost > threshold;
    Some(CheckAssessment {
        source: CheckAssessmentSource::SessionPriceOutlier,
        check_id: CHECK_ID.to_owned(),
        status: if is_finding { "finding" } else { "clean" }.to_owned(),
        persisted: None,
        result: Some(json!({
            "outcome": if is_finding { "finding" } else { "clean" },
            "label": if is_finding { "Higher than usual" } else { "Within the cohort" },
            "estimated_cost_usd": target_cost,
            "threshold_usd": threshold,
            "priced_session_count": costs.len(),
        })),
    })
}

fn is_target(candidate: &SessionIdentity, target: &SessionIdentity) -> bool {
    candidate.environment_key == target.environment_key
        && candidate.agent == target.agent
        && candidate.session_id == target.session_id
}

fn max_age_seconds(policy: PriceOutlierConfig) -> Option<i64> {
    let seconds = policy.max_age_hours.checked_mul(60)?.checked_mul(60)?;
    i64::try_from(seconds).ok()
}

fn not_assessed(reason: &str) -> CheckAssessment {
    not_assessed_with_sample(reason, 0)
}

fn not_assessed_with_sample(reason: &str, sample_size: usize) -> CheckAssessment {
    CheckAssessment {
        source: CheckAssessmentSource::SessionPriceOutlier,
        check_id: CHECK_ID.to_owned(),
        status: "not_assessed".to_owned(),
        persisted: None,
        result: Some(json!({
            "outcome": "not_assessed",
            "reason": reason,
            "priced_session_count": sample_size,
        })),
    }
}

#[cfg(test)]
fn row_model_json(tokens: u64) -> (String, String) {
    let breakdown = serde_json::json!({
        "test-model": {
            "input_tokens": tokens,
            "output_tokens": 0,
            "cache_read_tokens": 0,
            "cache_creation_tokens": 0,
            "cache_creation_1h_tokens": 0
        }
    })
    .to_string();
    (breakdown.clone(), breakdown)
}

#[cfg(test)]
fn insert_session(
    connection: &Connection,
    environment: &str,
    agent: &str,
    session_id: &str,
    updated_at_epoch: Option<i64>,
) {
    connection
        .execute(
            "INSERT INTO session (environment_key, agent, session_id, updated_at_epoch)
             VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![environment, agent, session_id, updated_at_epoch],
        )
        .unwrap();
}

#[cfg(test)]
fn insert_analysis(
    connection: &Connection,
    environment: &str,
    agent: &str,
    session_id: &str,
    tokens: u64,
) {
    let (models, pricing) = row_model_json(tokens);
    connection
        .execute(
            "INSERT INTO session_analysis (
                environment_key, agent, session_id, model_breakdown_json,
                pricing_breakdown_json
             ) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT (environment_key, agent, session_id) DO UPDATE SET
                model_breakdown_json = excluded.model_breakdown_json,
                pricing_breakdown_json = excluded.pricing_breakdown_json",
            rusqlite::params![environment, agent, session_id, models, pricing],
        )
        .unwrap();
}

#[cfg(test)]
fn create_database(connection: &Connection) {
    connection
        .execute_batch(
            "CREATE TABLE session (
                environment_key TEXT NOT NULL,
                agent TEXT NOT NULL,
                session_id TEXT NOT NULL,
                updated_at_epoch INTEGER,
                PRIMARY KEY (environment_key, agent, session_id)
             );
             CREATE TABLE session_analysis (
                environment_key TEXT NOT NULL,
                agent TEXT NOT NULL,
                session_id TEXT NOT NULL,
                model_breakdown_json TEXT NOT NULL,
                pricing_breakdown_json TEXT NOT NULL,
                PRIMARY KEY (environment_key, agent, session_id)
             );",
        )
        .unwrap();
}

#[cfg(test)]
fn target_identity(session_id: &str) -> SessionIdentity {
    SessionIdentity {
        environment_key: "native".to_owned(),
        agent: "pi".to_owned(),
        session_id: session_id.to_owned(),
    }
}

#[cfg(test)]
fn test_config() -> Config {
    Config::defaults()
}

#[cfg(test)]
fn price_snapshot() -> String {
    serde_json::json!({
        "schema": 2,
        "source": "models.dev",
        "version": "2026-10-16",
        "fetched_at": "2026-10-16T00:00:00Z",
        "etag": null,
        "models": {
            "test-model": {
                "input_cost_per_token": 0.000001,
                "output_cost_per_token": 0.0,
                "cache_read_cost_per_token": 0.0,
                "cache_write_cost_per_token": 0.0
            }
        }
    })
    .to_string()
}

#[cfg(test)]
fn insert_neighbors(connection: &Connection, target_epoch: i64, count: usize, tokens: u64) {
    for index in 0..count {
        let session_id = format!("neighbor-{index:02}");
        insert_session(
            connection,
            "native",
            "pi",
            &session_id,
            Some(target_epoch + index as i64),
        );
        insert_analysis(connection, "native", "pi", &session_id, tokens);
    }
}

#[cfg(test)]
fn with_fixture(test: impl FnOnce(&Connection, &Path, &SessionIdentity, &Config)) {
    use std::fs;
    use tempfile::tempdir;

    let temp = tempdir().unwrap();
    let database_path = temp.path().join("antiburn.sqlite3");
    let connection = Connection::open(&database_path).unwrap();
    create_database(&connection);
    let identity = target_identity("target");
    insert_session(
        &connection,
        &identity.environment_key,
        &identity.agent,
        &identity.session_id,
        Some(1_000_000),
    );
    insert_analysis(
        &connection,
        &identity.environment_key,
        &identity.agent,
        &identity.session_id,
        4_000_000,
    );
    insert_neighbors(&connection, 1_000_000, 7, 1_000_000);
    fs::write(temp.path().join("model-pricing.json"), price_snapshot()).unwrap();
    test(&connection, &database_path, &identity, &test_config());
}
