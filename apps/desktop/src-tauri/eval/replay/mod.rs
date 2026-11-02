#[path = "../support/capture.rs"]
mod capture;
mod cases;
#[path = "../support/selection.rs"]
mod selection;
mod support;

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};

use crate::jev::client::TypeSafeClient;
use crate::jev::decision_evaluator::TypeSafeDecisionEvaluator;
use support::{
    CASE_TIMEOUT, CheckedEvaluator, ScriptedEvaluator, SessionReplay, check_integrity,
    expected_value, limits, status_write,
};

#[tokio::test]
async fn scripted_cases() -> Result<()> {
    let mut ids = BTreeSet::new();
    for case in cases::cases() {
        ensure!(ids.insert(case.id), "duplicate replay case ID");
        let mut session = SessionReplay::for_case(&case).with_context(|| case.id)?;
        let evaluator = Arc::new(CheckedEvaluator::new(
            &case,
            ScriptedEvaluator::new(case.expected),
        ));
        let report = tokio::time::timeout(
            CASE_TIMEOUT,
            session.run(evaluator.clone(), limits(&case), || false),
        )
        .await??;
        let stored = session.snapshot()?;
        check_integrity(&case, &report, &stored)
            .with_context(|| format!("{}: {report:?}", case.id))?;
        ensure!(
            evaluator.inner.exhausted(),
            "{}: unused scripted answers",
            case.id
        );
        ensure!(
            evaluator
                .inputs
                .lock()
                .expect("input mutex is not poisoned")
                .len()
                == case.expected.len(),
            "scripted call count differs from the declared trace"
        );
        let mut status = None;
        for (assessment, expected) in stored["assessments"]
            .as_array()
            .context("assessments")?
            .iter()
            .zip(case.expected)
        {
            ensure!(
                assessment["answer"]["decisions"][0]["answers"][0]["result"]["output"]
                    == expected.label,
                "scripted answer was not retained"
            );
            ensure!(
                status_write(assessment)? == expected.status_write,
                "scripted action write differs"
            );
            if expected.status_write.is_some() {
                status = expected.status_write;
            }
            ensure!(
                assessment["actions"]["current_status"] == serde_json::to_value(status)?,
                "planned status differs from the last write"
            );
        }
    }
    Ok(())
}

fn selection_budget(cases: &[cases::Case]) -> Result<(usize, usize)> {
    let evaluations = cases
        .iter()
        .map(|case| limits(case).evaluations)
        .sum::<usize>();
    let attempts = cases
        .iter()
        .map(|case| limits(case).attempts)
        .sum::<usize>();
    ensure!(
        cases.len() <= 6 && evaluations <= 7 && attempts <= 14,
        "live selection exceeds synthetic diagnostic bounds"
    );
    Ok((evaluations, attempts))
}

#[tokio::test]
async fn replay_snapshot_keeps_observed_models_per_attempt_and_preserves_preparations() -> Result<()>
{
    use antiburn_checks::decision::DecisionAttempt;
    use antiburn_local::analysis::jev::{JevResponseObservation, JevUsage, NATIVE_MODEL_ALIAS};

    let case = cases::cases()
        .into_iter()
        .find(|case| case.id == "rolling-recovery")
        .context("two-assessment fixture")?;
    let mut session = SessionReplay::for_case(&case)?;
    let evaluator = Arc::new(CheckedEvaluator::new(
        &case,
        ScriptedEvaluator::new(case.expected),
    ));
    session.run(evaluator, limits(&case), || false).await?;
    let saved = session.store.lock().prepare(
        "SELECT candidate_id, preparation_json FROM burn_check_candidate ORDER BY trigger_position",
    )?.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    ensure!(saved.len() == 2, "fixture must retain two preparations");
    for ((candidate, preparation), model) in saved.iter().zip(["jev-1.13.0", "future/jev-2"]) {
        let prepared: Value = serde_json::from_str(preparation)?;
        ensure!(
            prepared["decisions"][0]["request"]["model"] == NATIVE_MODEL_ALIAS,
            "prepared request must keep the native alias"
        );
        session.store.record_burn_check_decision_observation(
            candidate,
            &DecisionAttempt::new("review", 1)?,
            &session.activation.id.definition_id,
            ("typesafe-systemone", NATIVE_MODEL_ALIAS),
            Some(JevResponseObservation {
                usage: JevUsage {
                    input_tokens: 100,
                    output_tokens: 4,
                },
                served_model: Some(model.into()),
            }),
            100,
        )?;
    }
    let snapshot = session.snapshot()?;
    for ((candidate, preparation), model) in saved.iter().zip(["jev-1.13.0", "future/jev-2"]) {
        let attempt = snapshot["attempts"]
            .as_array()
            .context("attempts")?
            .iter()
            .find(|attempt| attempt["candidate_id"] == *candidate)
            .context("stored attempt")?;
        ensure!(
            attempt["served_model"] == model && attempt["usage_known"] == true,
            "snapshot lost per-attempt identity"
        );
        let retained: String = session.store.lock().query_row(
            "SELECT preparation_json FROM burn_check_candidate WHERE candidate_id = ?1",
            [candidate],
            |row| row.get(0),
        )?;
        ensure!(
            retained == *preparation,
            "observation changed immutable preparation"
        );
    }
    ensure!(
        snapshot["usage"]["confirmedCalls"] == 2 && snapshot["usage"]["unknownOutcomes"] == 0,
        "known unpriced usage must stay confirmed"
    );
    ensure!(
        snapshot["usage"]["estimatedUsd"].is_null(),
        "future observed model must stay unpriced"
    );
    Ok(())
}

#[test]
fn entire_registry_fits_live_budget() -> Result<()> {
    let cases = cases::cases();
    let (evaluations, attempts) = selection_budget(&cases)?;
    ensure!(
        evaluations == 7 && attempts == 14,
        "registry budget changed"
    );
    for case in cases {
        ensure!(
            case.fixture.results.len() <= limits(&case).events,
            "{} exceeds its event budget",
            case.id
        );
    }
    Ok(())
}

#[tokio::test]
#[ignore = "Authorized bounded synthetic live diagnostic; requires TYPESAFE_API_KEY"]
async fn live() -> Result<()> {
    let suite = selection::suite();
    ensure!(suite == "development", "unknown replay suite: {suite}");
    let cases = selection::select(cases::cases(), |case| case.id);
    let (evaluations, attempts) = selection_budget(&cases)?;
    capture::report(
        "replay",
        &suite,
        &json!({"stage":"selected", "cases":cases.iter().map(expected_value).collect::<Vec<_>>(),
        "maximum_evaluations":evaluations, "maximum_attempts":attempts, "maximum_execution_seconds":u64::try_from(cases.len())? * CASE_TIMEOUT.as_secs()}),
    );
    let client = live_client()?;
    let mut rows = Vec::new();
    for case in &cases {
        let row = live_case(case, &client).await;
        let stopped = row["error"].is_string();
        rows.push(row);
        if stopped {
            break;
        }
    }
    let failed = rows.iter().any(|row| row["error"].is_string());
    capture::report(
        "replay",
        &suite,
        &json!({"stage":"summary", "selected_cases":cases.iter().map(|case|case.id).collect::<Vec<_>>(),
        "unexecuted_cases":cases.len()-rows.len(), "failed":failed, "cases":rows}),
    );
    ensure!(!failed, "live replay stopped; inspect the captured summary");
    Ok(())
}

fn live_client() -> Result<TypeSafeClient> {
    let provider = match std::env::var("ANTIBURN_EVAL_PROVIDER") {
        Ok(provider) => provider,
        Err(std::env::VarError::NotPresent) => "jev".into(),
        Err(error) => return Err(error.into()),
    };
    ensure!(
        provider == "jev",
        "the compiled replay route supports only the jev provider"
    );
    ensure!(
        std::env::var_os("ANTIBURN_EVAL_MODEL").is_none(),
        "replay uses its selected Jev connection; YAML does not select the model"
    );
    TypeSafeClient::new(
        std::env::var("TYPESAFE_API_KEY").context("set TYPESAFE_API_KEY for live replay")?,
    )
    .map_err(|error| anyhow::anyhow!("provider setup failed: {error:?}"))
}

async fn live_case(case: &cases::Case, client: &TypeSafeClient) -> Value {
    let mut row = expected_value(case);
    let started = Instant::now();
    let mut session = match SessionReplay::for_case(case) {
        Ok(session) => session,
        Err(error) => {
            row["error"] = json!(format!("source setup: {error:#}"));
            return row;
        }
    };
    let diagnostics = Arc::new(Mutex::new(Vec::new()));
    let captured = diagnostics.clone();
    let cancel_at = Instant::now() + CASE_TIMEOUT - std::time::Duration::from_secs(1);
    let usage_store = session.store.clone();
    let check_id = session.activation.id.definition_id.clone();
    let evaluator = Arc::new(CheckedEvaluator::new(
        case,
        TypeSafeDecisionEvaluator::new(client.clone())
            .with_cancellation(move || Instant::now() >= cancel_at)
            .with_usage(Arc::new(move |candidate, attempt, usage| {
                usage_store
                    .record_burn_check_decision_observation(
                        candidate,
                        attempt,
                        &check_id,
                        (
                            "typesafe-systemone",
                            antiburn_local::analysis::jev::NATIVE_MODEL_ALIAS,
                        ),
                        usage,
                        time::OffsetDateTime::now_utc().unix_timestamp(),
                    )
                    .map_err(|_| "Could not record replay provider usage.".to_owned())
            }))
            .with_diagnostics(Arc::new(move |record| {
                captured
                    .lock()
                    .expect("provider diagnostic mutex is not poisoned")
                    .push(support::attempt_value(record));
            })),
    ));
    let run = tokio::time::timeout(
        CASE_TIMEOUT,
        session.run(evaluator.clone(), limits(case), || {
            Instant::now() >= cancel_at
        }),
    )
    .await;
    let stored = session.snapshot();
    let result = match (run, &stored) {
        (Ok(Ok(report)), Ok(stored)) => {
            row["report"] = support::report_value(&report);
            row["report"]["provider"] = json!("typesafe-systemone");
            row["report"]["requested_model"] =
                json!(antiburn_local::analysis::jev::NATIVE_MODEL_ALIAS);
            check_integrity(case, &report, stored).and_then(|()| {
                let calls = stored["usage"]["confirmedCalls"]
                    .as_u64()
                    .context("confirmed provider usage")?;
                let recorded = diagnostics
                    .lock()
                    .expect("provider diagnostic mutex is not poisoned")
                    .len();
                ensure!(
                    recorded == report.attempts
                        && calls >= u64::try_from(report.evaluations)?
                        && calls <= u64::try_from(report.attempts)?,
                    "provider call counts differ from accepted replay work"
                );
                Ok(())
            })
        }
        (Ok(Err(error)), _) => Err(error),
        (Err(_), _) => Err(anyhow::anyhow!("case deadline exceeded")),
        (_, Err(error)) => Err(anyhow::anyhow!("store snapshot: {error:#}")),
    };
    match stored {
        Ok(stored) => row["store"] = stored,
        Err(error) => row["snapshot_error"] = json!(format!("{error:#}")),
    }
    if let Err(error) = result {
        row["error"] = json!(format!("{error:#}"));
    }
    row["inputs"] = json!(
        *evaluator
            .inputs
            .lock()
            .expect("input mutex is not poisoned")
    );
    row["provider_attempts"] = json!(
        *diagnostics
            .lock()
            .expect("provider diagnostic mutex is not poisoned")
    );
    row["latency_ms"] = json!(started.elapsed().as_secs_f64() * 1000.0);
    row
}
