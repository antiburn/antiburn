//! Measures provider throughput with published native session evidence.

use super::CHECK;
use crate::jev::worker::JevCheckDescriptor;
use crate::scope_creep_worker::tests::NativeFixture;
use crate::store::BurnCheckCandidate;
use antiburn_local::analysis::jev::JevCheck;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

const BACKGROUND_RECORDS: usize = 25_000;

fn large_session(
    index: usize,
    background_records: usize,
) -> anyhow::Result<(NativeFixture, BurnCheckCandidate, Value)> {
    let mut timings = crate::jev::benchmark::FixtureTimings::new();
    let fixture = NativeFixture::new(0);
    timings.checkpoint("fixture_initialization");
    let mut background = Value::Null;
    let native_source_timings = crate::jev::benchmark::with_native_part_batch(
        &fixture.directory.path().join("opencode.db"),
        "scope",
        |batch| {
            background = batch.append_background(2, background_records)?;
            batch.append(
                background_records + 2,
                "user",
                json!({"type": "text", "text": "Verify the parser fix."}),
            )?;
            batch.append(
                background_records + 3,
                "assistant",
                json!({
                    "type": "tool", "tool": "bash", "callID": "workflow-0",
                    "state": {
                        "status": "completed",
                        "input": {"command": "cargo test parser\ncargo check"},
                        "output": "Parser tests fail. Check passes.",
                    },
                }),
            )?;
            batch.append(
                background_records + 5,
                "assistant",
                json!({"type": "text", "text": "Parser tests fail. Check passes."}),
            )?;
            Ok(())
        },
    )?;
    timings.checkpoint("native_source_build");
    fixture
        .store
        .set_check_enabled(antiburn_local::checks::DetectorId::SkillOpportunities, true)?;
    fixture
        .store
        .capture_burn_check_boundaries(&[CHECK.id()], 0)?;

    let workspace = fixture.directory.path().join("workspace");
    let skill_directory = workspace.join(".opencode/skills/review-0");
    std::fs::create_dir_all(&skill_directory)?;
    let skill = "---\nname: review-0\ndescription: |\n  1. Run `cargo test parser`.\n  2. Run `cargo check`.\n---\n";
    std::fs::write(skill_directory.join("SKILL.md"), skill)?;
    let workspace = workspace.canonicalize()?;
    timings.checkpoint("skill_inventory_and_enrollment_setup");

    let source = rusqlite::Connection::open(fixture.directory.path().join("opencode.db"))?;
    let (records, bytes): (usize, usize) = source.query_row(
        "SELECT COUNT(*), SUM(length(CAST(data AS BLOB))) FROM part",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    drop(source);
    timings.checkpoint("source_payload_measurement");
    let published = fixture.publish();
    timings.checkpoint("native_publication");
    fixture.store.lock().execute(
        "UPDATE session SET cwd = ?1 WHERE session_id = ?2",
        rusqlite::params![
            workspace.to_string_lossy(),
            published.session.key.session_id
        ],
    )?;
    fixture.store.enqueue_burn_checks_for_revisions(
        &[(CHECK.id(), CHECK.evaluator_revision())],
        super::unix_now(),
        7,
    )?;
    let candidate = fixture
        .store
        .burn_check_candidates_for_revision(
            CHECK.id(),
            &CHECK.evaluator_revision(),
            super::unix_now(),
            CHECK.policy().idle_secs,
            16,
        )?
        .into_iter()
        .find(|candidate| candidate.session.key == published.session.key)
        .ok_or_else(|| anyhow::anyhow!("Skills benchmark candidate was not admitted"))?;
    let candidate = fixture
        .store
        .enrolled_burn_check_candidate(&candidate, CHECK.id())?;
    timings.checkpoint("candidate_admission");

    let home = antiburn_local::paths::home_dir()
        .ok_or_else(|| anyhow::anyhow!("Benchmark requires a native home directory"))?;
    let config = crate::agent_config::ConfigContext::native(
        antiburn_local::model::AgentKind::OpenCode,
        &home,
        Some(workspace),
    );
    let snapshot = fixture
        .store
        .load_smart_check_inputs(
            &candidate.session.key,
            candidate.published_fence,
            candidate.source_generation,
            crate::smart_check_inputs::DetectorInput::SkillOpportunities,
        )
        .map_err(|error| anyhow::anyhow!("{error:?}"))?;
    let inputs = fixture
        .store
        .load_smart_check_skill_inputs(snapshot, &config)
        .map_err(|error| anyhow::anyhow!("{error:?}"))?;
    let inputs = inputs
        .for_candidate(&candidate)
        .map_err(|error| anyhow::anyhow!("{error:?}"))?;
    let inventory_count = inputs.inventory().skills().len();
    let check = inputs
        .check()
        .map_err(|error| anyhow::anyhow!("{error:?}"))?;
    let work_count = check.descriptor_count();
    let plan = check.prepare(&check.session_context())?;
    timings.checkpoint("input_preparation_and_validation");
    let metadata = json!({
        "setup_phase_ms": timings.finish(),
        "native_source_phase_ms": native_source_timings,
        "synthetic": true,
        "session": index,
        "source_format": "OpenCodeSqliteV2",
        "native_records": records,
        "source_part_bytes": bytes,
        "byte_measure": "UTF-8 native part JSON payloads",
        "source_bytes": std::fs::metadata(fixture.directory.path().join("opencode.db"))?.len(),
        "source_bytes_measure": "SQLite source database file, including schema and page overhead",
        "background_records": background_records,
        "background": background,
        "procedure_operations": 2,
        "work_count": work_count,
        "selected_work_items": plan.work_items.len(),
        "fixture_inventory_count": 1,
        "inventory_count": inventory_count,
        "fixture_inventory_bytes": skill.len(),
    });
    Ok((fixture, candidate, metadata))
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "Runs live provider throughput measurements"]
async fn benchmark_large_sessions() -> anyhow::Result<()> {
    let fixtures = Arc::new(Mutex::new(Vec::new()));
    let retained = Arc::clone(&fixtures);
    crate::jev::benchmark::run_benchmark(&CHECK, move |index| {
        let (fixture, candidate, metadata) =
            tokio::task::block_in_place(|| large_session(index, BACKGROUND_RECORDS))?;
        let store = fixture.store.clone();
        retained
            .lock()
            .map_err(|_| anyhow::anyhow!("Fixture lock failed"))?
            .push(fixture);
        Ok((store, candidate, metadata))
    })
    .await
}
