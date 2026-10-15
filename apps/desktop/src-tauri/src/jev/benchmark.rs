//! Measures native check adapters through the production worker and Store.

use super::config;
use super::worker::{
    CandidateExecution, JevCheckDescriptor, TestWorkerEffects, WorkerHandle, unix_now,
};
use crate::jev_cloudflare;
use crate::session_lifecycle::SessionEvents;
use crate::store::{BurnCheckCandidate, Store};
use serde_json::{Value, json};
use std::time::{Duration, Instant};

#[path = "../../eval/support/provider_config.rs"]
mod provider_config;

pub(crate) struct FixtureTimings {
    last: Instant,
    phases: serde_json::Map<String, Value>,
}

impl FixtureTimings {
    pub(crate) fn new() -> Self {
        Self {
            last: Instant::now(),
            phases: serde_json::Map::new(),
        }
    }

    pub(crate) fn checkpoint(&mut self, phase: &str) {
        let now = Instant::now();
        let milliseconds = now.duration_since(self.last).as_secs_f64() * 1000.0;
        println!("fixture phase={phase} elapsed_ms={milliseconds:.1}");
        self.phases.insert(phase.to_owned(), json!(milliseconds));
        self.last = now;
    }

    pub(crate) fn finish(self) -> Value {
        Value::Object(self.phases)
    }
}

pub(crate) struct NativePartBatch<'a> {
    transaction: rusqlite::Transaction<'a>,
    session_id: &'a str,
}

impl NativePartBatch<'_> {
    pub(crate) fn clear_session(&self) -> anyhow::Result<()> {
        self.transaction
            .execute("DELETE FROM part WHERE session_id = ?1", [self.session_id])?;
        self.transaction.execute(
            "DELETE FROM message WHERE session_id = ?1",
            [self.session_id],
        )?;
        Ok(())
    }

    pub(crate) fn append(&self, index: usize, role: &str, part: Value) -> anyhow::Result<()> {
        let message_id = format!("m{index}");
        self.transaction.execute(
            "INSERT INTO message VALUES (?1, ?2, ?3, ?3, ?4)",
            rusqlite::params![
                message_id,
                self.session_id,
                index + 1001,
                json!({"role": role, "modelID": "synthetic-parser-model"}).to_string()
            ],
        )?;
        self.transaction.execute(
            "INSERT INTO part VALUES (?1, ?2, ?3, ?4, ?4, ?5)",
            rusqlite::params![
                format!("p{index}"),
                message_id,
                self.session_id,
                index + 1001,
                part.to_string()
            ],
        )?;
        Ok(())
    }

    pub(crate) fn append_background(&self, start: usize, records: usize) -> anyhow::Result<Value> {
        fn payload(block: &str, bytes: usize) -> String {
            let mut text = block.repeat(bytes.div_ceil(block.len()));
            text.truncate(bytes);
            text
        }
        let code = "pub fn scan_token(source: &str, offset: usize) -> Result<Token, ParseError> {\n    let remaining = source.get(offset..).ok_or(ParseError::InvalidBoundary)?;\n    let width = remaining.chars().take_while(|ch| ch.is_alphanumeric()).map(char::len_utf8).sum();\n    Ok(Token { start: offset, end: offset + width })\n}\n";
        let small_output = payload(code, 8 * 1024);
        let medium_output = payload(code, 24 * 1024);
        let large_output = payload(code, 90 * 1024);
        let reasoning = payload(
            "Inspect token boundaries and compare the recorded span with the scanner test. Preserve UTF-8 offsets and existing parser behavior.\n",
            1_857,
        );
        let progress = payload(
            "The scanner result matches the recorded token span. The parser change preserves the error position and stays within the assigned module.\n",
            480,
        );
        let mut tools = 0;
        let mut tool_output_bytes = 0;
        let mut reasoning_records = 0;
        let mut episodes = 0;
        for record in 0..records {
            if record % 250 == 0 {
                episodes += 1;
            }
            let (role, part) = if record % 250 == 0 {
                (
                    "user",
                    json!({"type":"text","text":format!("Inspect scanner module {}. Report the token span behavior; do not modify other modules.", record / 250)}),
                )
            } else if record % 250 == 249 || record + 1 == records {
                (
                    "user",
                    json!({"type":"text","text":"The scanner investigation is complete. No further work is requested."}),
                )
            } else {
                let part = match record % 25 {
                    1 => json!({"type":"step-start","snapshot":format!("scanner-step-{record}")}),
                    22 => {
                        json!({"type":"step-finish","reason":"stop","cost":0,"tokens":{"input":2400,"output":180,"reasoning":0,"cache":{"read":0,"write":0}}})
                    }
                    0 | 2 | 9 => {
                        reasoning_records += 1;
                        json!({"type":"reasoning","text":reasoning})
                    }
                    3 | 4 | 6 | 8 | 10 | 12 | 14 | 18 => {
                        let output = match record % 25 {
                            14 => &medium_output,
                            18 => &large_output,
                            _ => &small_output,
                        };
                        tools += 1;
                        tool_output_bytes += output.len();
                        json!({"type":"tool","tool":"read","callID":format!("background-read-{record}"),
                            "state":{"status":"completed","input":{"filePath":format!("/synthetic/scanner/module-{record}.rs")},"output":output}})
                    }
                    _ => {
                        json!({"type":"text","text":format!("Scanner progress {record}: {progress}")})
                    }
                };
                ("assistant", part)
            };
            self.append(start + record, role, part)?;
        }
        Ok(json!({
            "records": records, "main_thread_episodes": episodes,
            "episode_record_limit": 250, "tool_records": tools,
            "read_records": tools,
            "read_output_bytes_total": tool_output_bytes,
            "read_output_bytes": [small_output.len(), medium_output.len(), large_output.len()],
            "reasoning_records": reasoning_records, "reasoning_body_bytes": reasoning.len(),
            "progress_body_bytes": progress.len(),
            "scope": "all background and target activity stays on the main thread",
        }))
    }
}

pub(crate) fn with_native_part_batch(
    path: &std::path::Path,
    session_id: &str,
    build: impl FnOnce(&NativePartBatch<'_>) -> anyhow::Result<()>,
) -> anyhow::Result<Value> {
    let mut timings = FixtureTimings::new();
    let mut source = rusqlite::Connection::open(path)?;
    let batch = NativePartBatch {
        transaction: source.transaction()?,
        session_id,
    };
    // OpenCode schema: anomalyco/opencode commit 51e310c9, session/session.sql.ts.
    batch.transaction.execute_batch(
        "CREATE INDEX IF NOT EXISTS session_parent_idx ON session(parent_id);
         CREATE INDEX IF NOT EXISTS message_session_time_created_id_idx ON message(session_id, time_created, id);
         CREATE INDEX IF NOT EXISTS part_message_id_id_idx ON part(message_id, id);
         CREATE INDEX IF NOT EXISTS part_session_idx ON part(session_id);",
    )?;
    timings.checkpoint("native_source_indexes");
    build(&batch)?;
    timings.checkpoint("native_source_records");
    batch.transaction.commit()?;
    timings.checkpoint("native_source_commit");
    Ok(timings.finish())
}

fn positive_environment(name: &str, default: usize, maximum: usize) -> anyhow::Result<usize> {
    let value = match std::env::var(name) {
        Ok(value) => value.parse::<usize>()?,
        Err(std::env::VarError::NotPresent) => default,
        Err(error) => return Err(error.into()),
    };
    anyhow::ensure!(
        (1..=maximum).contains(&value),
        "{name} must be between 1 and {maximum}"
    );
    Ok(value)
}

async fn execute(
    check: &'static dyn JevCheckDescriptor,
    store: &Store,
    candidate: &BurnCheckCandidate,
    handle: &WorkerHandle,
) -> anyhow::Result<()> {
    let effects = TestWorkerEffects::default();
    let events = SessionEvents::default();
    loop {
        let (client, generation) = handle.execution_client().ok_or_else(|| {
            anyhow::anyhow!("Provider is unavailable or authentication was rejected")
        })?;
        anyhow::ensure!(
            handle.key_is_current(generation),
            "Provider generation changed"
        );
        handle.start_candidate_turn(check.id(), &candidate.session.key);
        let result = check
            .run_candidate(CandidateExecution {
                effects: &effects,
                store,
                candidate,
                client,
                handle,
                key_generation: generation,
                events: &events,
            })
            .await;
        handle.end_candidate_turn(check.id(), &candidate.session.key);
        result?;
        let saved = store
            .burn_check_assessment(&candidate.session.key, check.id())?
            .ok_or_else(|| anyhow::anyhow!("Worker persisted no assessment"))?;
        if saved.status == "completed" {
            return Ok(());
        }
        let (category, next_attempt): (Option<String>, Option<i64>) = store.lock().query_row(
            "SELECT last_error_category, next_attempt_at_epoch FROM burn_check_assessment WHERE check_id = ?1 AND session_id = ?2 AND agent = ?3 AND environment_key = ?4",
            rusqlite::params![check.id(), candidate.session.key.session_id, candidate.session.key.agent, candidate.session.key.environment_key],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        if saved.status == "failed"
            && matches!(
                category.as_deref(),
                Some("sampling_incomplete" | "no_candidates")
            )
        {
            return Ok(());
        }
        anyhow::ensure!(
            category.as_deref() == Some("continuing"),
            "Assessment status={} error={category:?}",
            saved.status
        );
        let next_attempt =
            next_attempt.ok_or_else(|| anyhow::anyhow!("Continuation has no scheduled retry"))?;
        let delay = u64::try_from(next_attempt.saturating_sub(unix_now()).max(0))?;
        tokio::time::sleep(Duration::from_secs(delay)).await;
    }
}

pub(crate) async fn run_benchmark(
    check: &'static dyn JevCheckDescriptor,
    fixture: impl Fn(usize) -> anyhow::Result<(Store, BurnCheckCandidate, Value)>
    + Send
    + Sync
    + 'static,
) -> anyhow::Result<()> {
    let preset_name =
        std::env::var("ANTIBURN_BENCHMARK_PROVIDER").or_else(|error| match error {
            std::env::VarError::NotPresent => Ok("jev".to_owned()),
            error => Err(error),
        })?;
    let preset =
        provider_config::ProviderPreset::parse(&preset_name).map_err(anyhow::Error::msg)?;
    let model = match std::env::var("ANTIBURN_BENCHMARK_MODEL") {
        Ok(model) => Some(model),
        Err(std::env::VarError::NotPresent) => None,
        Err(error) => return Err(error.into()),
    };
    let connection = preset
        .environment_connection(model.as_deref())
        .map_err(anyhow::Error::msg)?;
    let credential = preset
        .environment_credential()
        .map_err(|error| anyhow::anyhow!("Provider credential: {error:?}"))?;
    let sessions = positive_environment("ANTIBURN_BENCHMARK_SESSIONS", 3, 1000)?;
    let timeout = Duration::from_secs(u64::try_from(positive_environment(
        "ANTIBURN_BENCHMARK_TIMEOUT_SECS",
        300,
        3600,
    )?)?);
    let handle = WorkerHandle::default();
    handle
        .set_system_one_connection(connection.clone(), credential)
        .map_err(|error| anyhow::anyhow!("Provider connection: {error:?}"))?;
    let mut rows = Vec::with_capacity(sessions);
    let mut completed = 0;
    let mut measured_seconds = 0.0;
    for index in 0..sessions {
        let indexing_started = Instant::now();
        let (store, candidate, metadata) = fixture(index)?;
        let indexing_ms = indexing_started.elapsed().as_secs_f64() * 1000.0;
        if let Some(saved) = store.burn_check_assessment(&candidate.session.key, check.id())? {
            anyhow::ensure!(
                saved.status == "queued" && saved.result_json.is_none() && saved.request_count == 0,
                "Fixture must not contain a prebuilt result or prior execution"
            );
        }
        let before = store.burn_check_usage_summary()?;
        let started = Instant::now();
        let result =
            tokio::time::timeout(timeout, execute(check, &store, &candidate, &handle)).await;
        let elapsed = started.elapsed().as_secs_f64();
        measured_seconds += elapsed;
        let mut error = match result {
            Ok(Ok(())) => None,
            Ok(Err(error)) => Some(format!("{error:#}")),
            Err(_) => Some(format!("Session exceeded {} seconds", timeout.as_secs())),
        };
        let saved = store.burn_check_assessment(&candidate.session.key, check.id())?;
        let durable_completed = saved
            .as_ref()
            .is_some_and(|saved| saved.status == "completed");
        let terminal_partial = saved
            .as_ref()
            .is_some_and(|saved| saved.status == "failed" && saved.result_json.is_some());
        let no_candidates = if saved.as_ref().is_some_and(|saved| saved.status == "failed") {
            let category: Option<String> = store.lock().query_row(
                "SELECT last_error_category FROM burn_check_assessment WHERE check_id = ?1 AND session_id = ?2 AND agent = ?3 AND environment_key = ?4",
                rusqlite::params![check.id(), candidate.session.key.session_id, candidate.session.key.agent, candidate.session.key.environment_key],
                |row| row.get(0),
            )?;
            category.as_deref() == Some("no_candidates")
        } else {
            false
        };
        let after = store.burn_check_usage_summary()?;
        let calls = after.confirmed_calls.saturating_sub(before.confirmed_calls);
        let unknown = after
            .unknown_outcomes
            .saturating_sub(before.unknown_outcomes);
        let publication = saved
            .as_ref()
            .and_then(|saved| saved.result_json.as_deref())
            .map(serde_json::from_str::<Value>)
            .transpose()?;
        let findings = publication
            .as_ref()
            .and_then(|value| {
                value
                    .get("findings")
                    .or_else(|| value.pointer("/assessment/findings"))
            })
            .and_then(Value::as_array)
            .map(Vec::len);
        let request_count = saved.as_ref().map(|saved| saved.request_count);
        let zero_calls = calls == 0 && unknown == 0;
        if error.is_none() {
            error = if !durable_completed && !terminal_partial && !no_candidates {
                Some("Worker did not complete the durable assessment".to_owned())
            } else if unknown > 0 {
                Some("Assessment has unknown provider outcomes".to_owned())
            } else if findings.is_none() && !no_candidates {
                Some("Assessment has no durable findings array".to_owned())
            } else {
                None
            };
        }
        let is_completed = error.is_none();
        completed += usize::from(is_completed);
        let row = json!({
            "session": index, "fixture": metadata, "indexing_ms": indexing_ms,
            "latency_ms": elapsed * 1000.0, "completed": is_completed,
            "status": saved.as_ref().map(|saved| &saved.status), "durable_completed": durable_completed,
            "request_count": request_count,
            "input_tokens": after.input_tokens.saturating_sub(before.input_tokens),
            "output_tokens": after.output_tokens.saturating_sub(before.output_tokens),
            "confirmed_calls": calls, "cache_hits": after.cache_hits.saturating_sub(before.cache_hits),
            "unknown_outcomes": unknown, "zero_calls": zero_calls,
            "no_candidates": no_candidates,
            "findings": findings, "error": error,
            "coverage": publication.as_ref().and_then(|value| value.get("coverage").or_else(|| value.pointer("/assessment/coverage"))),
            "decisions": publication.as_ref().and_then(|value| value.get("decisions").or_else(|| value.pointer("/assessment/decisions"))).and_then(Value::as_array).map(|decisions| decisions.iter().map(|decision| json!({"outcome": decision.get("outcome").or_else(|| decision.get("status")), "judgments": decision.get("judgments")})).collect::<Vec<_>>()),
        });
        println!(
            "benchmark check={} session={} latency_ms={:.1} indexing_ms={:.1} completed={} calls={} findings={findings:?} error={:?}",
            check.id(),
            index,
            elapsed * 1000.0,
            indexing_ms,
            is_completed,
            calls,
            row["error"]
        );
        rows.push(row);
    }
    let report = json!({
        "check": check.id(), "evaluator_revision": check.evaluator_revision(),
        "preset": preset, "connection": connection, "sessions": rows,
        "completed_sessions": completed, "errors": sessions - completed,
        "measurement_seconds": measured_seconds,
        "indexing_measurement": "Entire fixture callback: payload and companion-file construction, native SQLite writes, fingerprinting, real parser visit, fenced Store publication, candidate admission, local input preparation/assertions, thread/runtime setup, and fixture retention; not parser-only time or native session discovery",
        "latency_measurement": "Admitted candidate execution through durable assessment, including input reload, production transport, and continuation waits; excludes fixture setup and scheduler wake/discovery/debounce waits",
        "completed_sessions_per_second": completed as f64 / measured_seconds,
        "timeout_seconds": timeout.as_secs(),
    });
    println!(
        "benchmark check={} completed={completed}/{sessions} completion_throughput={:.4} sessions/s errors={}",
        check.id(),
        completed as f64 / measured_seconds,
        sessions - completed
    );
    println!("{}", serde_json::to_string_pretty(&report)?);
    anyhow::ensure!(
        completed == sessions,
        "{} of {sessions} benchmark sessions failed",
        sessions - completed
    );
    Ok(())
}
