use super::*;
use crate::analysis::{NativeEvidenceFact, NativeMessageObservation};
use serde_json::json;

struct Journal {
    _directory: tempfile::TempDir,
    path: std::path::PathBuf,
    connection: Connection,
    sequence: u64,
}

impl Journal {
    fn new(users: usize) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("journal.db");
        let connection = Connection::open(&path).unwrap();
        connection.execute_batch("CREATE TABLE session(id TEXT PRIMARY KEY, parent_id TEXT);
            INSERT INTO session VALUES ('root', NULL);
            CREATE TABLE message(id TEXT PRIMARY KEY, session_id TEXT, data TEXT);
            CREATE TABLE part(id TEXT PRIMARY KEY, message_id TEXT, session_id TEXT, data TEXT);
            CREATE TABLE event_sequence(aggregate_id TEXT PRIMARY KEY, seq INTEGER, owner_id TEXT);
            CREATE TABLE event(id TEXT PRIMARY KEY, aggregate_id TEXT, seq INTEGER, type TEXT, data TEXT);
            CREATE UNIQUE INDEX event_aggregate_seq_idx ON event(aggregate_id, seq);").unwrap();
        let mut journal = Self {
            _directory: directory,
            path,
            connection,
            sequence: 0,
        };
        journal.append("session.created.1", json!({"info":{"id":"root"}}));
        for index in 0..users {
            journal.append(
                "message.updated.1",
                json!({"info":{
                    "id":format!("user-{index}"), "role":"user", "sessionID":"root"
                }}),
            );
        }
        journal
    }

    fn append(&mut self, kind: &str, mut data: Value) {
        data["sessionID"] = json!("root");
        let sequence = self.sequence;
        self.connection
            .execute(
                "INSERT INTO event VALUES (?1, 'root', ?2, ?3, ?4)",
                params![
                    format!("event-{sequence}"),
                    sequence,
                    kind,
                    data.to_string()
                ],
            )
            .unwrap();
        self.connection
            .execute(
                "INSERT INTO event_sequence VALUES ('root', ?1, NULL)
            ON CONFLICT(aggregate_id) DO UPDATE SET seq = excluded.seq",
                [sequence],
            )
            .unwrap();
        self.sequence += 1;
    }

    fn assistant(&mut self, id: &str) {
        self.append(
            "message.updated.1",
            json!({"info":{
                "id":id, "sessionID":"root", "role":"assistant", "parentID":"user-0"
            }}),
        );
    }

    fn result(&mut self, id: &str) {
        self.assistant(id);
        for status in ["running", "completed"] {
            self.append(
                "message.part.updated.1",
                json!({"part":{
                    "id":format!("part-{id}"), "sessionID":"root", "messageID":id,
                    "type":"tool", "callID":format!("call-{id}"), "tool":"bash",
                    "state":{"status":status, "input":{"command":"true"}, "output":"ok",
                        "title":"Synthetic result", "metadata":{}, "time":{"start":1,"end":2}}
                }}),
            );
        }
    }

    fn read(&self, cursor: Option<&OpenCodeJournalCursor>) -> OpenCodeToolResultPage {
        read_opencode_live_tool_result_page(&self.path, "root", cursor, None, &|| false).unwrap()
    }

    fn drain(&self, mut cursor: OpenCodeJournalCursor) -> Vec<OpenCodeToolResultPage> {
        let mut pages = Vec::new();
        loop {
            let page = self.read(Some(&cursor));
            assert!(page.evidence.events.len() <= LIVE_RESULTS);
            assert!(serde_json::to_vec(&page.evidence).unwrap().len() <= MAX_TOOL_RESULT_BYTES);
            cursor = serde_json::from_slice(&serde_json::to_vec(&page.cursor).unwrap()).unwrap();
            let more = page.has_more;
            pages.push(page);
            if !more {
                return pages;
            }
        }
    }
}

fn failure(page: &OpenCodeToolResultPage) -> ToolResultCompleteness {
    assert_eq!(page.evidence.events.len(), 1);
    let NativeEvidenceFact::AdmissionFailure(failure) = &page.evidence.events[0].fact else {
        panic!("unsafe admission must replace unusable page facts")
    };
    assert_eq!(failure.session_id, "root");
    assert!(!page.evidence.history_complete);
    failure.reason
}

#[test]
fn tool_only_assistant_owner_conflict_emits_admission_without_another_part_event() {
    let mut journal = Journal::new(1);
    let seed = journal.read(None);
    journal.result("assistant");
    let first = journal.read(Some(&seed.cursor));
    assert!(first.evidence.events.iter().any(|event| matches!(
        event.fact,
        NativeEvidenceFact::Message(NativeMessageObservation::Public(_))
    )));
    assert_eq!(first.results.len(), 2);
    journal.append(
        "message.updated.1",
        json!({"info":{
            "id":"assistant", "role":"assistant", "sessionID":"other", "parentID":"user-0"
        }}),
    );
    let page = journal.read(Some(&first.cursor));
    assert!(page.results.is_empty());
    assert_eq!(failure(&page), ToolResultCompleteness::UnsafeIdentity);
    assert_eq!(page.cursor.through_sequence, journal.sequence - 1);
    let restored = serde_json::from_slice(&serde_json::to_vec(&page.cursor).unwrap()).unwrap();
    let idle = journal.read(Some(&restored));
    assert_eq!(idle.live_input_events, 0);
    assert_eq!(failure(&idle), ToolResultCompleteness::UnsafeIdentity);
}

fn evict_assistant_header(
    journal: &mut Journal,
    cursor: OpenCodeJournalCursor,
) -> OpenCodeJournalCursor {
    for index in 0..140 {
        journal.assistant(&format!("later-assistant-{index}"));
    }
    let pages = journal.drain(cursor);
    assert!(pages.iter().all(|page| page.history_complete));
    assert!(
        pages
            .iter()
            .flat_map(|page| &page.evidence.events)
            .all(|event| !matches!(event.fact, NativeEvidenceFact::AdmissionFailure(_)))
    );
    let cursor = pages.last().unwrap().cursor.clone();
    let encoded = serde_json::to_value(&cursor).unwrap();
    assert!(
        encoded["live"]["requests"]["messages"]
            .get("assistant")
            .is_none()
    );
    assert_eq!(
        encoded["live"]["parts"]["part-assistant"]["unsafe_ownership"],
        false
    );
    cursor
}

#[test]
fn evicted_assistant_header_role_conflict_uses_retained_tool_ownership_after_restore() {
    let mut journal = Journal::new(1);
    let seed = journal.read(None);
    journal.result("assistant");
    let first = journal.read(Some(&seed.cursor));
    let cursor = evict_assistant_header(&mut journal, first.cursor);
    let idle = journal.read(Some(&cursor));
    assert!(idle.evidence.events.is_empty() && idle.results.is_empty());
    journal.append(
        "message.updated.1",
        json!({"info":{
            "id":"assistant", "role":"user", "sessionID":"root", "parentID":"user-0"
        }}),
    );
    let page = journal.read(Some(&idle.cursor));
    assert!(page.results.is_empty());
    assert_eq!(failure(&page), ToolResultCompleteness::UnsafeOwnership);
    let live = page.cursor.live.as_ref().unwrap();
    assert!(live.requests.main_session() && live.history_safe);
    assert!(live.parts["part-assistant"].unsafe_ownership);
    let restored = serde_json::from_slice(&serde_json::to_vec(&page.cursor).unwrap()).unwrap();
    let idle = journal.read(Some(&restored));
    assert_eq!(idle.live_input_events, 0);
    assert_eq!(failure(&idle), ToolResultCompleteness::UnsafeOwnership);
}

#[test]
fn retained_owner_conflict_still_publishes_when_a_later_part_evicts_its_proof_in_the_same_page() {
    let mut journal = Journal::new(1);
    let seed = journal.read(None);
    journal.result("assistant");
    let first = journal.read(Some(&seed.cursor));
    for index in 0..LIVE_PARTS - 1 {
        journal.result(&format!("retained-{index}"));
    }
    let pages = journal.drain(first.cursor);
    let cursor = evict_assistant_header(&mut journal, pages.last().unwrap().cursor.clone());
    assert_eq!(cursor.live.as_ref().unwrap().parts.len(), LIVE_PARTS);
    journal.append(
        "message.updated.1",
        json!({"info":{
            "id":"assistant", "role":"user", "sessionID":"root", "parentID":"user-0"
        }}),
    );
    journal.result("new-owner");
    let page = journal.read(Some(&cursor));
    assert_eq!(failure(&page), ToolResultCompleteness::UnsafeOwnership);
    assert!(!page.has_more);
    assert!(
        !page
            .cursor
            .live
            .as_ref()
            .unwrap()
            .parts
            .contains_key("part-assistant")
    );
}

#[test]
fn evicted_assistant_header_and_validated_removal_keep_unrelated_suffix_results_admitted() {
    let mut journal = Journal::new(1);
    let seed = journal.read(None);
    journal.result("assistant");
    let first = journal.read(Some(&seed.cursor));
    let cursor = evict_assistant_header(&mut journal, first.cursor);
    journal.append("message.removed.1", json!({"messageID":"assistant"}));
    let page = journal.read(Some(&cursor));
    assert!(
        page.evidence
            .events
            .iter()
            .any(|event| matches!(&event.fact,
        NativeEvidenceFact::MessageRemoval(removal) if removal.message_id == "assistant"))
    );
    assert!(
        !page
            .evidence
            .events
            .iter()
            .any(|event| matches!(event.fact, NativeEvidenceFact::AdmissionFailure(_)))
    );
    assert!(page.cursor.live.as_ref().unwrap().parts["part-assistant"].unsafe_ownership);
    let restored = serde_json::from_slice(&serde_json::to_vec(&page.cursor).unwrap()).unwrap();
    let idle = journal.read(Some(&restored));
    assert!(idle.evidence.events.is_empty() && idle.results.is_empty());
    journal.result("suffix");
    let pages = journal.drain(idle.cursor);
    assert!(
        pages
            .iter()
            .flat_map(|page| &page.results)
            .any(|result| result.finality
                && result.admission() == crate::analysis::ToolResultAdmission::Admitted)
    );
    assert!(
        pages
            .iter()
            .flat_map(|page| &page.evidence.events)
            .all(|event| !matches!(event.fact, NativeEvidenceFact::AdmissionFailure(_)))
    );
}

#[test]
fn retained_unsafe_flags_emit_markers_without_new_saved_fields() {
    let journal = Journal::new(1);
    let seed = journal.read(None);
    for (field, value, reason) in [
        (
            "unsafe_identity",
            true,
            ToolResultCompleteness::UnsafeIdentity,
        ),
        ("root", false, ToolResultCompleteness::UnsafeOwnership),
        ("history_safe", false, ToolResultCompleteness::UnsafeOrder),
    ] {
        let mut encoded = serde_json::to_value(&seed.cursor).unwrap();
        if field == "history_safe" {
            encoded["live"][field] = json!(value);
        } else {
            encoded["live"]["requests"][field] = json!(value);
        }
        let cursor = serde_json::from_value(encoded.clone()).unwrap();
        let page = journal.read(Some(&cursor));
        assert_eq!(failure(&page), reason);
        assert_eq!(serde_json::to_value(&page.cursor).unwrap(), encoded);
    }
}

#[test]
fn public_count_byte_and_tombstone_losses_keep_result_admission_and_do_not_replay() {
    for limit in ["count", "bytes", "tombstones"] {
        let mut journal = Journal::new(if limit == "tombstones" { 64 } else { 1 });
        let seed = journal.read(None);
        match limit {
            "count" => {
                for index in 0..140 {
                    journal.assistant(&format!("assistant-{index}"));
                }
            }
            "bytes" => {
                for index in 0..16 {
                    let id = format!("assistant-{index}");
                    journal.assistant(&id);
                    journal.append(
                        "message.part.updated.1",
                        json!({"part":{
                            "id":format!("text-{index}"), "sessionID":"root", "messageID":id,
                            "type":"text", "text":"x".repeat(16 * 1024), "time":{"end":2}
                        }}),
                    );
                    journal.append(
                        "message.updated.1",
                        json!({"info":{
                            "id":id, "sessionID":"root", "role":"assistant", "parentID":"user-0",
                            "time":{"completed":3}
                        }}),
                    );
                }
            }
            "tombstones" => {
                journal.assistant("assistant");
                for index in 0..65 {
                    let id = format!("step-{index}");
                    journal.append("message.part.updated.1", json!({"part":{
                        "id":id, "sessionID":"root", "messageID":"assistant", "type":"step-start"
                    }}));
                    journal.append(
                        "message.part.removed.1",
                        json!({
                            "messageID":"assistant", "partID":id
                        }),
                    );
                }
            }
            _ => unreachable!(),
        }
        let pages = journal.drain(seed.cursor);
        assert!(pages.iter().all(|page| page.history_complete), "{limit}");
        let last = pages.last().unwrap();
        assert!(!last.evidence.history_complete, "{limit}");
        assert!(
            pages
                .iter()
                .flat_map(|page| &page.evidence.events)
                .all(|event| !matches!(event.fact, NativeEvidenceFact::AdmissionFailure(_))),
            "{limit}"
        );
        let cursor = last.cursor.clone();
        let idle = journal.read(Some(&cursor));
        assert!(
            idle.evidence.events.is_empty() && idle.results.is_empty(),
            "{limit}"
        );
        assert!(
            !idle.evidence.history_complete && idle.history_complete,
            "{limit}"
        );
        journal.result("suffix");
        let suffix = journal.drain(idle.cursor);
        let result = suffix
            .iter()
            .flat_map(|page| &page.results)
            .find(|result| result.finality)
            .unwrap();
        assert_eq!(
            result.admission(),
            crate::analysis::ToolResultAdmission::Admitted,
            "{limit}"
        );
        assert!(
            suffix.iter().all(|page| !page.evidence.history_complete),
            "{limit}"
        );
        assert!(
            journal
                .read(Some(&suffix.last().unwrap().cursor))
                .evidence
                .events
                .is_empty(),
            "{limit}"
        );
    }
}
