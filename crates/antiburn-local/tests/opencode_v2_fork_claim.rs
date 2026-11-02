use antiburn_local::analysis::{
    NormalizedRecord, RawSource, RecordSink, SessionInput, SessionSummary, SourceChangedReason,
    SourceFormat, VisitOutcome, reader_for_input,
};
use antiburn_local::discovery::agents::opencode;

#[derive(Default)]
struct Sink {
    records: usize,
}

impl RecordSink for Sink {
    fn record(&mut self, _: NormalizedRecord) {
        self.records += 1;
    }

    fn finish(&mut self, _: SessionSummary) {}
}

#[test]
fn changed_external_fork_boundary_rejects_the_claim_before_emitting_records() {
    for mutation in [
        "UPDATE session_message SET seq = 6 WHERE id = 'msg_boundary'",
        "DELETE FROM session_message WHERE id = 'msg_boundary'",
        "DELETE FROM session_message WHERE id = 'msg_boundary';
         INSERT INTO session_message VALUES ('msg_boundary', 'ses_parent', 'assistant', 9, 1000, 1000, '{}')",
    ] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("opencode.db");
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection.execute_batch(include_str!(
            "../src/discovery/agents/tests/fixtures/opencode_pinned_mixed.sql"
        )).unwrap();
        connection.execute_batch(
            "INSERT INTO session_v2 (id, project_id, slug, directory, version, time_created, time_updated)
               VALUES ('ses_parent', 'global', 'parent', '/repo', '2.0.32', 1000, 1000),
                      ('ses_fork', 'global', 'fork', '/repo', '2.0.32', 1000, 1000);
             UPDATE session_v2 SET fork_session_id = 'ses_parent',
               fork_boundary = '{\"type\":\"through\",\"messageID\":\"msg_boundary\"}' WHERE id = 'ses_fork';
             INSERT INTO session_message VALUES
               ('msg_boundary', 'ses_parent', 'assistant', 4, 1000, 1000, '{}'),
               ('msg_new', 'ses_fork', 'assistant', 5, 2000, 2000,
                '{\"time\":{\"created\":2000},\"model\":{\"id\":\"m\",\"providerID\":\"p\"},\"tokens\":{\"input\":1,\"output\":1,\"reasoning\":0,\"cache\":{\"read\":0,\"write\":0}},\"content\":[]}');"
        ).unwrap();
        let (_, (latest, rows)) =
            opencode::db_session_fingerprint_snapshot(&path, "ses_fork", &|| false).unwrap();
        let fingerprint = format!("sv1:db:{latest}:{rows}");
        let input = SessionInput {
            agent: "opencode".into(), session_id: "ses_fork".into(),
            source: RawSource::Sqlite(path), source_format: SourceFormat::OpenCodeSqliteV2,
            fork_parent_session_id: Some("ses_parent".into()),
        };
        connection.execute_batch(mutation).unwrap();
        let mut sink = Sink::default();
        let outcome = reader_for_input(&input)
            .visit_db_claimed(&input, &fingerprint, &|| false, &mut sink).unwrap();
        assert_eq!(outcome, VisitOutcome::SourceChanged(SourceChangedReason::FingerprintMismatch));
        assert_eq!(sink.records, 0, "{mutation}");
    }
}
