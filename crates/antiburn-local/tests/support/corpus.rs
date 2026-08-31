//! Deterministic synthetic Claude-shaped JSONL corpus generator.
//!
//! Shared by the integration tests and the bench targets (via `#[path]`
//! inclusion), so both measure the same corpus tiers. Everything here is
//! synthetic and fictional: no real project names, paths, prompts, or
//! transcripts. The generator is seeded and deterministic — the same
//! `SessionSpec` always yields byte-identical JSONL.
//!
//! The record-type mix follows the shape of a housekeeping frequency table
//! (a few dominant conversational types plus a long tail of rare
//! housekeeping/eventless types), with entirely fictional values.
#![allow(dead_code)]

use std::path::Path;

use serde_json::{Value, json};

/// Fictional models rotated at fixed session fractions (cap-safe at any scale).
const MODELS: [&str; 3] = [
    "claude-3-5-haiku-20241022",
    "claude-sonnet-4-6",
    "claude-opus-4-6",
];

const SKILLS: [&str; 3] = ["orbit-planner", "atlas-index", "tide-charter"];
const MCP_SERVERS: [&str; 2] = ["nebula-docs", "lunar-data"];
const FILE_TOOLS: [&str; 3] = ["Read", "Grep", "Edit"];
const FICTIONAL_PATHS: [&str; 3] = [
    "/home/avery/projects/demo-app/src/orbit.rs",
    "/home/avery/projects/demo-app/src/atlas.rs",
    "/home/avery/projects/demo-app/notes/tides.md",
];
/// Fictional long-tail housekeeping types the adapter does not recognize.
const UNRECOGNIZED_TYPES: [&str; 2] = ["relay_probe", "shelf_audit"];

/// splitmix64 — small, deterministic, dependency-free.
pub struct CorpusRng(u64);

impl CorpusRng {
    pub fn new(seed: u64) -> Self {
        Self(seed.wrapping_add(0x9E37_79B9_7F4A_7C15))
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform value in `0..bound` (bound > 0).
    pub fn pick(&mut self, bound: u64) -> u64 {
        self.next_u64() % bound
    }
}

/// One synthetic session to generate.
pub struct SessionSpec {
    pub seed: u64,
    pub session_index: usize,
    pub records: usize,
    /// Marks assistant turns as sidechain (a delegated/subagent transcript).
    pub delegated: bool,
    /// Number of `Task` tool_use spawns to plant near the session start.
    pub task_spawns: usize,
    /// Insert one fictional inert unknown record every N records.
    /// Evidence-bearing insertion wins when both intervals match.
    pub unrecognized_every: Option<usize>,
    /// Insert one fictional evidence-bearing unknown every N records.
    /// This insertion runs before the inert insertion at matching indexes.
    pub evidence_bearing_unrecognized_every: Option<usize>,
    /// Replace the record at this index with a single oversized line of
    /// roughly `oversized_bytes` bytes.
    pub oversized_at: Option<usize>,
    pub oversized_bytes: usize,
    /// Reuses assistant message ids with this modulus when set.
    pub message_id_modulus: Option<usize>,
    /// Adds a synthetic chained `uuid` and `parentUuid` identity to each record.
    pub thread_identity: bool,
}

impl SessionSpec {
    pub fn tier_s(seed: u64, session_index: usize, records: usize) -> Self {
        Self {
            seed,
            session_index,
            records,
            delegated: false,
            task_spawns: 0,
            unrecognized_every: None,
            evidence_bearing_unrecognized_every: None,
            oversized_at: None,
            oversized_bytes: 0,
            message_id_modulus: None,
            thread_identity: false,
        }
    }
}

/// Expected record tallies, so tests can assert outcome shape exactly.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Tallies {
    pub total_records: usize,
    /// Assistant records; every one carries `message.usage`.
    pub assistant_records: usize,
    /// User records (plain prompts and tool_result carriers).
    pub user_records: usize,
    /// Recognized eventless housekeeping (attachments, summaries, snapshots,
    /// compaction boundaries).
    pub eventless_records: usize,
    pub unrecognized_records: usize,
    pub evidence_bearing_unrecognized_records: usize,
    pub oversized_records: usize,
    pub task_spawns: usize,
    pub compaction_boundaries: usize,
}

pub struct GeneratedSession {
    pub session_id: String,
    pub jsonl: String,
    pub tallies: Tallies,
}

/// Generates one Claude-shaped JSONL session, deterministically.
pub fn generate_session(spec: &SessionSpec) -> GeneratedSession {
    let mut rng = CorpusRng::new(
        spec.seed
            .wrapping_mul(0x0100_0000_01B3)
            .wrapping_add(spec.session_index as u64),
    );
    let session_id = format!("synthetic-{:04}-{:08x}", spec.session_index, spec.seed);
    let mut jsonl = String::with_capacity(spec.records * 320);
    let mut tallies = Tallies::default();
    let base_epoch: i64 = 1_770_000_000 + (spec.session_index as i64) * 86_400;

    // Fixed-position structure: bounded regardless of scale, so evidence
    // caps (models, compactions) are never exceeded by tier size alone.
    let compaction_points = fixed_points(spec.records, &[2, 4], 5);
    let model_switch_points = fixed_points(spec.records, &[1, 2], 3);
    let mut model_index = 0usize;
    let mut planted_spawns = 0usize;
    let mut last_tool_id: Option<String> = None;

    for index in 0..spec.records {
        let ts = base_epoch + (index as i64) * 7;
        let push_record = |jsonl: &mut String, value: &Value| {
            push_record(jsonl, value, &session_id, index, spec.thread_identity);
        };

        if spec.oversized_at == Some(index) {
            jsonl.push_str(&oversized_record(ts, index, spec.oversized_bytes));
            jsonl.push('\n');
            tallies.total_records += 1;
            tallies.oversized_records += 1;
            continue;
        }
        if let Some(every) = spec.evidence_bearing_unrecognized_every
            && every > 0
            && index > 0
            && index.is_multiple_of(every)
        {
            let kind = UNRECOGNIZED_TYPES[index / every % UNRECOGNIZED_TYPES.len()];
            push_record(
                &mut jsonl,
                &json!({
                    "type": kind,
                    "role": "agent",
                    "timestamp": ts,
                    "usage": {"input_tokens": 13, "output_tokens": 5}
                }),
            );
            tallies.total_records += 1;
            tallies.evidence_bearing_unrecognized_records += 1;
            continue;
        }
        if let Some(every) = spec.unrecognized_every
            && every > 0
            && index > 0
            && index.is_multiple_of(every)
        {
            let kind = UNRECOGNIZED_TYPES[index / every % UNRECOGNIZED_TYPES.len()];
            push_record(
                &mut jsonl,
                &json!({"type": kind, "timestamp": ts, "payload": {"sweep": index}}),
            );
            tallies.total_records += 1;
            tallies.unrecognized_records += 1;
            continue;
        }
        if compaction_points.contains(&index) {
            push_record(
                &mut jsonl,
                &json!({
                    "type": "system",
                    "subtype": "compact_boundary",
                    "timestamp": ts,
                    "content": "The synthetic session was compacted."
                }),
            );
            tallies.total_records += 1;
            tallies.eventless_records += 1;
            tallies.compaction_boundaries += 1;
            continue;
        }
        if model_switch_points.contains(&index) {
            model_index = (model_index + 1) % MODELS.len();
        }

        if planted_spawns < spec.task_spawns && index >= 1 {
            push_record(
                &mut jsonl,
                &assistant_record(
                    ts,
                    &message_id(spec, index, &session_id),
                    MODELS[model_index],
                    spec.delegated,
                    &mut rng,
                    json!([{
                        "type": "tool_use",
                        "id": format!("tool-{session_id}-{index}"),
                        "name": "Task",
                        "input": {
                            "description": "Inspect the fictional orbit module.",
                            "prompt": "Read the fictional module and report its shape."
                        }
                    }]),
                ),
            );
            tallies.total_records += 1;
            tallies.assistant_records += 1;
            tallies.task_spawns += 1;
            planted_spawns += 1;
            continue;
        }

        let roll = rng.pick(100);
        match roll {
            // Dominant band: assistant text turns with usage.
            0..=54 => {
                push_record(
                    &mut jsonl,
                    &assistant_record(
                        ts,
                        &message_id(spec, index, &session_id),
                        MODELS[model_index],
                        spec.delegated,
                        &mut rng,
                        json!([{
                            "type": "text",
                            "text": format!("Synthetic turn {index} for the fictional demo app.")
                        }]),
                    ),
                );
                tallies.assistant_records += 1;
            }
            // User prompts.
            55..=69 => {
                push_record(
                    &mut jsonl,
                    &json!({
                        "type": "user",
                        "timestamp": ts,
                        "message": {
                            "role": "user",
                            "content": format!("Inspect the fictional module number {index}.")
                        }
                    }),
                );
                tallies.user_records += 1;
            }
            // Assistant tool_use turns.
            70..=81 => {
                let tool_id = format!("tool-{session_id}-{index}");
                let content = match rng.pick(4) {
                    0 => json!([{
                        "type": "tool_use",
                        "id": tool_id,
                        "name": "Skill",
                        "input": {"skill": SKILLS[index % SKILLS.len()]}
                    }]),
                    1 => json!([{
                        "type": "tool_use",
                        "id": tool_id,
                        "name": format!(
                            "mcp__{}__search_docs",
                            MCP_SERVERS[index % MCP_SERVERS.len()]
                        ),
                        "input": {"query": "synthetic orbit"}
                    }]),
                    _ => json!([{
                        "type": "tool_use",
                        "id": tool_id,
                        "name": FILE_TOOLS[index % FILE_TOOLS.len()],
                        "input": {"file_path": FICTIONAL_PATHS[index % FICTIONAL_PATHS.len()]}
                    }]),
                };
                last_tool_id = Some(format!("tool-{session_id}-{index}"));
                push_record(
                    &mut jsonl,
                    &assistant_record(
                        ts,
                        &message_id(spec, index, &session_id),
                        MODELS[model_index],
                        spec.delegated,
                        &mut rng,
                        content,
                    ),
                );
                tallies.assistant_records += 1;
            }
            // User tool_result carriers.
            82..=86 => {
                let tool_id = last_tool_id
                    .clone()
                    .unwrap_or_else(|| format!("tool-{session_id}-none"));
                push_record(
                    &mut jsonl,
                    &json!({
                        "type": "user",
                        "timestamp": ts,
                        "message": {
                            "role": "user",
                            "content": [{
                                "type": "tool_result",
                                "tool_use_id": tool_id,
                                "is_error": rng.pick(4) == 0,
                                "content": "The fictional module report is ready."
                            }]
                        }
                    }),
                );
                tallies.user_records += 1;
            }
            // Assistant thinking turns.
            87..=91 => {
                push_record(
                    &mut jsonl,
                    &assistant_record(
                        ts,
                        &message_id(spec, index, &session_id),
                        MODELS[model_index],
                        spec.delegated,
                        &mut rng,
                        json!([
                            {"type": "thinking", "thinking": "I will inspect the fictional module."},
                            {"type": "text", "text": format!("Synthetic reasoned turn {index}.")}
                        ]),
                    ),
                );
                tallies.assistant_records += 1;
            }
            // Housekeeping tail: recognized eventless record types.
            92..=94 => {
                push_record(
                    &mut jsonl,
                    &json!({
                        "type": "attachment",
                        "timestamp": ts,
                        "attachment": {
                            "type": "skill_listing",
                            "content": "- orbit-planner: Plans synthetic orbital work.\n- atlas-index: Indexes synthetic atlas notes."
                        }
                    }),
                );
                tallies.eventless_records += 1;
            }
            95..=96 => {
                push_record(
                    &mut jsonl,
                    &json!({
                        "type": "attachment",
                        "timestamp": ts,
                        "attachment": {
                            "type": "mcp_instructions_delta",
                            "addedNames": [MCP_SERVERS[index % MCP_SERVERS.len()]],
                            "addedBlocks": ["Search synthetic nebula documentation."]
                        }
                    }),
                );
                tallies.eventless_records += 1;
            }
            97..=98 => {
                push_record(
                    &mut jsonl,
                    &json!({
                        "type": "summary",
                        "summary": format!("Synthetic summary {index} of fictional work."),
                        "leafUuid": format!("leaf-{session_id}-{index}")
                    }),
                );
                tallies.eventless_records += 1;
            }
            _ => {
                push_record(
                    &mut jsonl,
                    &json!({
                        "type": "file-history-snapshot",
                        "timestamp": ts,
                        "snapshot": {"path": FICTIONAL_PATHS[index % FICTIONAL_PATHS.len()]}
                    }),
                );
                tallies.eventless_records += 1;
            }
        }
        tallies.total_records += 1;
    }

    GeneratedSession {
        session_id,
        jsonl,
        tallies,
    }
}

/// Grows a session until its JSONL is at least `target_bytes` long.
pub fn generate_session_of_bytes(
    seed: u64,
    session_index: usize,
    target_bytes: usize,
) -> GeneratedSession {
    generate_session_of_bytes_with_options(seed, session_index, target_bytes, None, false)
}

/// Grows a session with synthetic chained record identities.
pub fn generate_session_of_bytes_with_identity(
    seed: u64,
    session_index: usize,
    target_bytes: usize,
    message_id_modulus: Option<usize>,
) -> GeneratedSession {
    generate_session_of_bytes_with_options(
        seed,
        session_index,
        target_bytes,
        message_id_modulus,
        true,
    )
}

fn generate_session_of_bytes_with_options(
    seed: u64,
    session_index: usize,
    target_bytes: usize,
    message_id_modulus: Option<usize>,
    thread_identity: bool,
) -> GeneratedSession {
    let bytes_per_record = if thread_identity { 380 } else { 280 };
    let mut records = target_bytes / bytes_per_record;
    loop {
        let mut spec = SessionSpec::tier_s(seed, session_index, records);
        spec.message_id_modulus = message_id_modulus;
        spec.thread_identity = thread_identity;
        let session = generate_session(&spec);
        if session.jsonl.len() >= target_bytes {
            return session;
        }
        let deficit = target_bytes - session.jsonl.len();
        records += deficit / bytes_per_record + 64;
    }
}

fn message_id(spec: &SessionSpec, index: usize, session_id: &str) -> String {
    let id_index = spec
        .message_id_modulus
        .filter(|modulus| *modulus > 0)
        .map_or(index, |modulus| index % modulus);
    format!("msg-{session_id}-{id_index}")
}

fn assistant_record(
    ts: i64,
    message_id: &str,
    model: &str,
    delegated: bool,
    rng: &mut CorpusRng,
    content: Value,
) -> Value {
    let mut record = json!({
        "type": "assistant",
        "timestamp": ts,
        "message": {
            "id": message_id,
            "role": "assistant",
            "model": model,
            "usage": {
                "input_tokens": 20 + rng.pick(400),
                "output_tokens": 5 + rng.pick(120),
                "cache_read_input_tokens": rng.pick(30_000),
                "cache_creation_input_tokens": rng.pick(2_000)
            },
            "content": content
        }
    });
    if delegated {
        record["isSidechain"] = json!(true);
    }
    // A sparse tail of effort/speed annotations, like a real mode mix.
    if rng.pick(20) == 0 {
        record["effort"] = json!("high");
    }
    if rng.pick(20) == 0 {
        record["speed"] = json!("fast");
    }
    record
}

fn oversized_record(ts: i64, index: usize, bytes: usize) -> String {
    // One well-formed record whose single line is roughly `bytes` long.
    let filler = "orbit ".repeat(bytes / 6 + 1);
    format!(
        "{{\"type\":\"assistant\",\"timestamp\":{ts},\"message\":{{\"id\":\"msg-oversized-{index}\",\"role\":\"assistant\",\"model\":\"claude-3-5-haiku-20241022\",\"usage\":{{\"input_tokens\":2,\"output_tokens\":3}},\"content\":[{{\"type\":\"text\",\"text\":\"{filler}\"}}]}}}}"
    )
}

fn push_record(
    jsonl: &mut String,
    value: &Value,
    session_id: &str,
    index: usize,
    thread_identity: bool,
) {
    if !thread_identity {
        jsonl.push_str(&value.to_string());
        jsonl.push('\n');
        return;
    }
    let mut value = value.clone();
    value["uuid"] = json!(format!("uuid-{session_id}-{index}"));
    value["parentUuid"] = if index == 0 {
        Value::Null
    } else {
        json!(format!("uuid-{session_id}-{}", index - 1))
    };
    jsonl.push_str(&value.to_string());
    jsonl.push('\n');
}

/// Writes one synthetic OpenCode provider-DB session from the shared corpus.
pub fn write_provider_db(path: &Path, session: &GeneratedSession) -> anyhow::Result<()> {
    let mut connection = rusqlite::Connection::open(path)?;
    connection.execute_batch(
        "CREATE TABLE session (
             id TEXT PRIMARY KEY, parent_id TEXT, title TEXT,
             time_created INTEGER, time_updated INTEGER
         );
         CREATE TABLE message (
             id TEXT PRIMARY KEY, session_id TEXT, time_created INTEGER,
             time_updated INTEGER, data TEXT
         );
         CREATE TABLE part (
             id TEXT PRIMARY KEY, message_id TEXT, session_id TEXT,
             time_created INTEGER, time_updated INTEGER, data TEXT
         );
         CREATE INDEX session_parent_idx ON session (parent_id);
         CREATE INDEX message_session_time_created_id_idx
             ON message (session_id, time_created, id);
         CREATE INDEX part_message_id_id_idx ON part (message_id, id);
         CREATE INDEX part_session_idx ON part (session_id);",
    )?;
    let transaction = connection.transaction()?;
    {
        transaction.execute(
            "INSERT INTO session (id, parent_id, time_created, time_updated) VALUES (?1, NULL, 0, 0)",
            [&session.session_id],
        )?;
        let mut insert_message =
            transaction.prepare("INSERT INTO message VALUES (?1, ?2, ?3, ?3, ?4)")?;
        let mut insert_part =
            transaction.prepare("INSERT INTO part VALUES (?1, ?2, ?3, ?4, ?4, ?5)")?;
        for (index, line) in session.jsonl.lines().enumerate() {
            let Ok(record) = serde_json::from_str::<Value>(line) else {
                continue;
            };
            let record_type = record.get("type").and_then(Value::as_str);
            let message = record.get("message");
            let role = match record_type {
                Some("assistant") => "assistant",
                Some("user") => {
                    let has_tool_result = message
                        .and_then(|message| message.get("content"))
                        .and_then(Value::as_array)
                        .is_some_and(|content| {
                            content.iter().any(|part| {
                                part.get("type").and_then(Value::as_str) == Some("tool_result")
                            })
                        });
                    if has_tool_result { "tool" } else { "user" }
                }
                Some("system")
                    if record.get("subtype").and_then(Value::as_str)
                        == Some("compact_boundary") =>
                {
                    "system"
                }
                _ => continue,
            };
            let timestamp = record
                .get("timestamp")
                .and_then(Value::as_i64)
                .unwrap_or(index as i64);
            let message_id = format!("provider-message-{index}");
            let usage = message.and_then(|message| message.get("usage"));
            let data = json!({
                "role": role,
                "modelID": message.and_then(|message| message.get("model")),
                "variant": record.get("effort"),
                "tokens": {
                    "input": usage.and_then(|usage| usage.get("input_tokens")).and_then(Value::as_u64).unwrap_or(0),
                    "output": usage.and_then(|usage| usage.get("output_tokens")).and_then(Value::as_u64).unwrap_or(0),
                    "reasoning": 0,
                    "cache": {
                        "read": usage.and_then(|usage| usage.get("cache_read_input_tokens")).and_then(Value::as_u64).unwrap_or(0),
                        "write": usage.and_then(|usage| usage.get("cache_creation_input_tokens")).and_then(Value::as_u64).unwrap_or(0)
                    }
                }
            });
            insert_message.execute(rusqlite::params![
                message_id,
                session.session_id,
                timestamp,
                data.to_string()
            ])?;

            if role == "system" {
                insert_part.execute(rusqlite::params![
                    format!("provider-part-{index}-0"),
                    message_id,
                    session.session_id,
                    timestamp,
                    json!({"type": "compaction"}).to_string()
                ])?;
                continue;
            }
            let Some(content) = message
                .and_then(|message| message.get("content"))
                .and_then(Value::as_array)
            else {
                continue;
            };
            for (part_index, part) in content.iter().enumerate() {
                let part = match part.get("type").and_then(Value::as_str) {
                    Some("tool_use") => json!({
                        "type": "tool",
                        "tool": part.get("name"),
                        "state": {"input": part.get("input")}
                    }),
                    Some("thinking") => json!({"type": "reasoning"}),
                    Some("text") => json!({"type": "text"}),
                    Some("tool_result") => continue,
                    _ => continue,
                };
                insert_part.execute(rusqlite::params![
                    format!("provider-part-{index}-{part_index}"),
                    message_id,
                    session.session_id,
                    timestamp,
                    part.to_string()
                ])?;
            }
        }
    }
    transaction.commit()?;
    Ok(())
}

/// Fixed session-fraction positions, e.g. `&[2, 4]` of denominator 5 →
/// indices at 40% and 80%. Empty for very small sessions.
fn fixed_points(records: usize, numerators: &[usize], denominator: usize) -> Vec<usize> {
    if records < denominator * 2 {
        return Vec::new();
    }
    numerators
        .iter()
        .map(|numerator| records * numerator / denominator)
        .collect()
}
