use super::*;
use crate::insights_worker::journal::tests::{SyntheticJournal, run_page};
use crate::store::native_tool_results::source_delivery::opencode_v2::{
    OpenCodeV2AdmittedSource, OpenCodeV2CaptureClaim, OpenCodeV2CaptureStart,
};
use antiburn_local::analysis::opencode_v2_events::OpenCodeV2EventJoiner;

struct V2Source {
    claim: OpenCodeV2CaptureClaim,
    sequence: u64,
    joiner: OpenCodeV2EventJoiner,
}

impl V2Source {
    fn start(fixture: &Fixture) -> Self {
        let key = SessionKey::new("native", "opencode", "v2-hot");
        let mut record = fixture
            .store
            .session(&SessionKey::new("native", "pi", "main"))
            .unwrap()
            .unwrap();
        record.key = key.clone();
        record.source_kind = "providerDb".into();
        record.source_label = "opencode:v2-hot:open_code_v2_sqlite".into();
        let admitted_source = OpenCodeV2AdmittedSource {
            database_path: fixture.directory.path().join("opencode.db"),
            file_identity: "synthetic-database-identity".into(),
        };
        record.activity_cursor = format!(
            "opencode-admission-v2:{}",
            serde_json::to_string(&(
                &admitted_source.database_path,
                "v2-hot",
                &admitted_source.file_identity,
                "synthetic-scope"
            ))
            .unwrap()
        );
        fixture
            .store
            .upsert_sessions(&[record.clone()], &crate::agents::evidence_cohort())
            .unwrap();
        let (presence, _) = fixture
            .store
            .session_presence_for_keys(std::slice::from_ref(&key))
            .unwrap();
        let generation = fixture
            .store
            .source_generation_batch(std::slice::from_ref(&key))
            .unwrap()[0]
            .unwrap();
        let claim = fixture
            .store
            .start_opencode_v2_capture_epoch(
                &[OpenCodeV2CaptureStart {
                    key,
                    incarnation: presence[0].incarnation,
                    source_generation: generation,
                    source_label: record.source_label,
                    cwd: record.cwd.unwrap(),
                    admitted_source,
                    baseline_sequence: Some(0),
                }],
                "fairness-v2",
                time::OffsetDateTime::now_utc().unix_timestamp(),
                600,
            )
            .unwrap()
            .unwrap()
            .remove(0);
        let mut source = Self {
            claim,
            sequence: 0,
            joiner: OpenCodeV2EventJoiner::new(),
        };
        source.emit(&fixture.store, "session.inbox.enqueued", json!({"inboxID":"request", "item":{"type":"user", "delivery":"queue", "payload":{"text":"Inspect the build"}}}));
        source.emit(
            &fixture.store,
            "session.inbox.delivered",
            json!({"inboxID":"request"}),
        );
        source.emit(
            &fixture.store,
            "session.step.started",
            json!({"assistantMessageID":"step"}),
        );
        source
    }

    fn emit(&mut self, store: &Store, kind: &str, extra: Value) {
        self.sequence += 1;
        let mut data = json!({"sessionID":"v2-hot"});
        data.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        if matches!(kind, "session.tool.called" | "session.tool.failed") {
            data["executed"] = json!(false);
        }
        let event = json!({"id":format!("v2-event-{}", self.sequence), "type":kind, "created":1000,
            "durable":{"aggregateID":"v2-hot", "seq":self.sequence, "version":if kind == "session.tool.failed" {2} else {1}}, "data":data});
        let result = self
            .joiner
            .observe(&event)
            .map(|joined| joined.into_observation(false, true));
        assert!(
            store
                .advance_opencode_v2_capture(
                    &self.claim,
                    self.sequence,
                    result.as_ref(),
                    time::OffsetDateTime::now_utc().unix_timestamp(),
                    600
                )
                .unwrap()
        );
    }

    fn result(&mut self, store: &Store, call: usize) {
        self.emit(
            store,
            "session.tool.input.started",
            json!({"assistantMessageID":"step", "id":format!("call-{call}"), "name":"shell"}),
        );
        self.emit(store, "session.tool.called", json!({"assistantMessageID":"step", "id":format!("call-{call}"), "input":{"command":"cargo check"}}));
        self.emit(store, "session.tool.failed", json!({"assistantMessageID":"step", "id":format!("call-{call}"), "content":[{"type":"text", "text":"Synthetic build failure"}]}));
    }
}

#[tokio::test(start_paused = true)]
async fn hot_v2_work_rotates_with_file_and_multiple_definitions_without_a_clock_tick() {
    let fixture = Fixture::new();
    let source = CHECK.replace("cooldown: 60s", "cooldown: 0s");
    std::fs::write(fixture.global.join("failed-retry-loop.yml"), &source).unwrap();
    std::fs::write(
        fixture.global.join("second.yml"),
        source.replace("name: Synthetic retry review", "name: Second check"),
    )
    .unwrap();
    fixture
        .store
        .lock()
        .execute("DELETE FROM native_file_delivery", [])
        .unwrap();
    std::fs::write(
        fixture.directory.path().join("main.jsonl"),
        transcript("main", 0),
    )
    .unwrap();
    let mut journal = SyntheticJournal::new(16);
    journal.index(&fixture.store, fixture.project.to_str().unwrap());
    let mut host = fixture.host();
    host.reconcile(&Admission::Ready).unwrap();
    let mut v2 = V2Source::start(&fixture);
    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    assert!(run_page(&fixture.store, journal.source(), now).await);
    assert!(
        crate::insights_worker::file_live::process_next(&fixture.store, &|| now, &|_| {})
            .await
            .unwrap()
    );
    let mut attached = false;
    for _ in 0..8 {
        if !host.catch_up().unwrap().1 {
            attached = true;
            break;
        }
    }
    assert!(attached);
    for call in 0..120 {
        v2.result(&fixture.store, call);
    }
    for result in 0..3 {
        journal.start_result(result);
        journal.result(result);
    }
    journal.index(&fixture.store, fixture.project.to_str().unwrap());
    assert!(run_page(&fixture.store, journal.source(), now + 1).await);
    std::fs::write(
        fixture.directory.path().join("main.jsonl"),
        transcript("main", 3),
    )
    .unwrap();
    fixture.index("native", "main", 3);
    assert!(
        crate::insights_worker::file_live::process_next(&fixture.store, &|| now + 1, &|_| {})
            .await
            .unwrap()
    );
    let started = tokio::time::Instant::now();
    let mut productive = 0;
    for turn in 0..8 {
        for call in 120 + turn * 3..123 + turn * 3 {
            v2.result(&fixture.store, call);
        }
        productive += usize::from(host.catch_up().unwrap().1);
        for job in host.evaluation_jobs(4) {
            fixture.evaluator.answer("stuck");
            let _ = job.run().await.unwrap();
        }
    }
    assert_eq!(tokio::time::Instant::now(), started);
    assert!(productive > 0);
    let connection = fixture.store.lock();
    let reached: Vec<(String, usize, String)> = connection.prepare(
        "SELECT check_id, COUNT(DISTINCT branch_key), GROUP_CONCAT(DISTINCT branch_key) FROM burn_check_assessment_history GROUP BY check_id ORDER BY check_id",
    ).unwrap().query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?))).unwrap().collect::<rusqlite::Result<_>>().unwrap();
    assert_eq!(reached.len(), 2);
    assert!(
        reached.iter().all(|(_, branches, _)| *branches == 3),
        "{reached:?}"
    );
    let attempts: usize = connection
        .query_row(
            "SELECT COUNT(*) FROM burn_check_evaluation_attempt",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(attempts, fixture.evaluator.inputs.lock().unwrap().len());
    assert!((4..=32).contains(&attempts), "attempts={attempts}");
}
