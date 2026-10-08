//! Native parent links prove descent independently of the shared thread root.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

const MAX_RECORDS: usize = 16_384;
const MAX_BYTES: usize = 4 * 1024 * 1024;
const MAX_DEPTH: usize = 4096;
const MAX_ID_BYTES: usize = 512;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(super) struct ClaudeAncestry {
    records: BTreeMap<String, Option<ParentLink>>,
    bytes: usize,
    capped: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ParentLink {
    parent: Option<String>,
}

impl ClaudeAncestry {
    pub(super) fn observe(&mut self, record: &Value) {
        let Some(id) = native_id(record.get("uuid")) else {
            return;
        };
        let Some(key) = record_key(record, id) else {
            return;
        };
        if let Some(previous) = self.records.get_mut(&key) {
            if let Some(link) = previous.take() {
                self.bytes -= link.parent.as_ref().map_or(0, String::len);
            }
            return;
        }
        if self.capped {
            return;
        }
        let parent = match record.get("parentUuid") {
            Some(Value::Null) => record.get("logicalParentUuid").or(Some(&Value::Null)),
            None => record.get("logicalParentUuid"),
            value => value,
        };
        let link = match parent {
            Some(Value::Null) => Some(ParentLink { parent: None }),
            value => native_id(value)
                .and_then(|id| record_key(record, id))
                .map(|parent| ParentLink {
                    parent: Some(parent),
                }),
        };
        let bytes = key.len()
            + link
                .as_ref()
                .and_then(|l| l.parent.as_ref())
                .map_or(0, String::len);
        if self.records.len() >= MAX_RECORDS || self.bytes + bytes > MAX_BYTES {
            self.capped = true;
            return;
        }
        self.bytes += bytes;
        self.records.insert(key, link);
    }

    pub(super) fn descends_from(&self, record: &Value, ancestor: Option<&str>) -> bool {
        let Some(ancestor) = ancestor
            .filter(|id| !id.is_empty() && id.len() <= MAX_ID_BYTES)
            .and_then(|id| record_key(record, id))
        else {
            return false;
        };
        if !matches!(self.records.get(&ancestor), Some(Some(_))) {
            return false;
        }
        let Some(mut current) = native_id(record.get("uuid")).and_then(|id| record_key(record, id))
        else {
            return false;
        };
        if current == ancestor {
            return false;
        }
        let mut visited = BTreeSet::new();
        let mut found_ancestor = false;
        for _ in 0..MAX_DEPTH {
            if !visited.insert(current.clone()) {
                return false;
            }
            let Some(Some(ParentLink { parent })) = self.records.get(&current) else {
                return false;
            };
            found_ancestor |= current == ancestor;
            let Some(parent) = parent else {
                return found_ancestor;
            };
            current.clone_from(parent);
        }
        false
    }
}

fn native_id(value: Option<&Value>) -> Option<&str> {
    value?
        .as_str()
        .filter(|id| !id.is_empty() && id.len() <= MAX_ID_BYTES)
}

fn record_key(record: &Value, id: &str) -> Option<String> {
    let context_id = |key| match record.get(key) {
        None => Some(""),
        value => native_id(value),
    };
    let session = context_id("sessionId")?;
    let agent = context_id("agentId")?;
    let sidechain = match record.get("isSidechain") {
        None => false,
        Some(value) => value.as_bool()?,
    };
    Some(
        serde_json::to_string(&(session, agent, sidechain, id))
            .expect("native identity tuple serializes"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn record(id: &str, parent: Value) -> Value {
        json!({"uuid":id, "parentUuid":parent})
    }

    #[test]
    fn ancestry_requires_a_complete_cycle_free_chain_and_survives_serialization() {
        let mut ancestry = ClaudeAncestry::default();
        for node in [
            record("root", Value::Null),
            record("call", json!("root")),
            record("bridge", json!("call")),
            record("sibling", json!("root")),
            record("result", json!("bridge")),
        ] {
            ancestry.observe(&node);
        }
        let restored: ClaudeAncestry =
            postcard::from_bytes(&postcard::to_allocvec(&ancestry).unwrap()).unwrap();
        assert!(restored.descends_from(&record("result", json!("bridge")), Some("call")));
        assert!(!restored.descends_from(&record("sibling", json!("root")), Some("call")));
        assert!(!restored.descends_from(&record("call", json!("root")), Some("call")));
        assert!(!restored.descends_from(&json!({"parentUuid":"call"}), Some("call")));
        assert!(!restored.descends_from(&record("result", json!("bridge")), None));
        for parent in [json!("missing"), json!("result"), json!(42)] {
            let mut ancestry = ClaudeAncestry::default();
            ancestry.observe(&record("call", parent));
            ancestry.observe(&record("result", json!("call")));
            assert!(!ancestry.descends_from(&record("result", json!("call")), Some("call")));
        }
        ancestry.observe(&record("bridge", json!("sibling")));
        assert!(!ancestry.descends_from(&record("result", json!("bridge")), Some("call")));
    }

    #[test]
    fn logical_parent_links_are_explicit_and_invalid_primary_links_do_not_fall_back() {
        let mut ancestry = ClaudeAncestry::default();
        ancestry.observe(&record("call", Value::Null));
        let boundary = json!({"uuid":"boundary", "parentUuid":null, "logicalParentUuid":"call"});
        ancestry.observe(&boundary);
        let result = record("result", json!("boundary"));
        ancestry.observe(&result);
        assert!(ancestry.descends_from(&result, Some("call")));
        let invalid = json!({"uuid":"invalid", "parentUuid":42, "logicalParentUuid":"call"});
        ancestry.observe(&invalid);
        assert!(!ancestry.descends_from(&invalid, Some("call")));
    }

    #[test]
    fn ancestry_depth_record_identity_and_byte_limits_fail_closed() {
        let mut ancestry = ClaudeAncestry::default();
        ancestry.observe(&record("call", Value::Null));
        let mut parent = "call".to_owned();
        for index in 0..MAX_DEPTH {
            let id = format!("node-{index}");
            ancestry.observe(&record(&id, json!(parent)));
            parent = id;
        }
        assert!(!ancestry.descends_from(&record(&parent, Value::Null), Some("call")));
        for index in MAX_DEPTH..MAX_RECORDS {
            ancestry.observe(&record(&format!("node-{index}"), Value::Null));
        }
        let overflow = record("overflow", json!("call"));
        ancestry.observe(&overflow);
        assert_eq!(ancestry.records.len(), MAX_RECORDS);
        assert!(!ancestry.descends_from(&overflow, Some("call")));
        assert!(!ancestry.descends_from(
            &record(&"x".repeat(MAX_ID_BYTES + 1), json!("call")),
            Some("call")
        ));
        let mut full = ClaudeAncestry {
            bytes: MAX_BYTES,
            ..Default::default()
        };
        full.observe(&record("call", Value::Null));
        assert!(full.records.is_empty());
    }
}
