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
    /// Insert one fictional unrecognized housekeeping record every N records.
    pub unrecognized_every: Option<usize>,
    /// Replace the record at this index with a single oversized line of
    /// roughly `oversized_bytes` bytes.
    pub oversized_at: Option<usize>,
    pub oversized_bytes: usize,
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
            oversized_at: None,
            oversized_bytes: 0,
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

        if spec.oversized_at == Some(index) {
            jsonl.push_str(&oversized_record(ts, index, spec.oversized_bytes));
            jsonl.push('\n');
            tallies.total_records += 1;
            tallies.oversized_records += 1;
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
                    &session_id,
                    index,
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
                        &session_id,
                        index,
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
                        &session_id,
                        index,
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
                        &session_id,
                        index,
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
    // Average record size is ~300 bytes; overshoot the first guess, then top up.
    let mut records = target_bytes / 280;
    loop {
        let session = generate_session(&SessionSpec::tier_s(seed, session_index, records));
        if session.jsonl.len() >= target_bytes {
            return session;
        }
        let deficit = target_bytes - session.jsonl.len();
        records += deficit / 280 + 64;
    }
}

fn assistant_record(
    ts: i64,
    session_id: &str,
    index: usize,
    model: &str,
    delegated: bool,
    rng: &mut CorpusRng,
    content: Value,
) -> Value {
    let mut record = json!({
        "type": "assistant",
        "timestamp": ts,
        "message": {
            "id": format!("msg-{session_id}-{index}"),
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

fn push_record(jsonl: &mut String, value: &Value) {
    jsonl.push_str(&value.to_string());
    jsonl.push('\n');
}

/// Writes one synthetic provider-DB (SQLite) session for the generic
/// schema-agnostic table walk: one JSON record per row in a `turns` table,
/// plus a non-JSON `housekeeping` table the walk must skip. Content comes
/// from the same deterministic generator as the JSONL tiers.
pub fn write_provider_db(path: &Path, session: &GeneratedSession) -> anyhow::Result<()> {
    let mut connection = rusqlite::Connection::open(path)?;
    connection.execute_batch(
        "CREATE TABLE turns (id INTEGER PRIMARY KEY, recorded_at INTEGER, payload TEXT);\n         CREATE TABLE housekeeping (id INTEGER PRIMARY KEY, note TEXT);",
    )?;
    let transaction = connection.transaction()?;
    {
        let mut insert =
            transaction.prepare("INSERT INTO turns (recorded_at, payload) VALUES (?1, ?2)")?;
        for (index, line) in session.jsonl.lines().enumerate() {
            insert.execute(rusqlite::params![index as i64, line])?;
        }
        let mut note = transaction.prepare("INSERT INTO housekeeping (note) VALUES (?1)")?;
        note.execute(["vacuum sweep of the fictional shelf index"])?;
        note.execute(["synthetic retention note for the demo app"])?;
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
