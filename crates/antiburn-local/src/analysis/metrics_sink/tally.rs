use std::collections::HashMap;
use std::mem::size_of;

use crate::analysis::model::{EventSource, Usage};

/// Names keep enough bytes for provider and tool identifiers.
pub(crate) const MAX_NAME_BYTES: usize = 64;
/// Skill display names retain realistic long command names.
pub(crate) const MAX_SKILL_NAME_BYTES: usize = 192;
/// Skill display names match the description-map cardinality.
pub(crate) const MAX_SKILL_NAMES: usize = 64;
/// The skill list covers more invocations than the chart can display clearly.
pub(crate) const MAX_SKILL_USES: usize = 256;
/// Late candidates cover more delayed commands than a normal transcript emits.
pub(crate) const MAX_LATE_CANDIDATES: usize = 256;
/// Tool-name counts cover twice the evidence-side name limit.
pub(crate) const MAX_TOOL_NAMES: usize = 256;
/// MCP server counts allow twice the evidence-side source limit.
pub(crate) const MAX_MCP_SERVERS: usize = 128;
/// Model attribution matches the evidence-side model limit.
pub(crate) const MAX_MODELS: usize = 32;
/// Model runs match the model-attribution limit.
pub(crate) const MAX_MODEL_RUNS: usize = 32;
/// Thinking modes cover more values than known providers emit.
pub(crate) const MAX_THINKING_MODES: usize = 64;
/// Speed values cover more values than known providers emit.
pub(crate) const MAX_SPEEDS: usize = 64;
/// Provisional built-in commands cannot consume the late-skill budget.
pub(crate) const MAX_BUILTIN_LATE_CANDIDATES: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct NameId(pub(crate) u16);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct IdentityKey {
    first: u64,
    second: u64,
    length: usize,
}

impl IdentityKey {
    pub(crate) fn new(value: &str) -> Self {
        Self {
            first: hash_bytes(value.as_bytes(), 0xcbf2_9ce4_8422_2325),
            second: hash_bytes(value.as_bytes(), 0x8422_2325_cbf2_9ce4),
            length: value.len(),
        }
    }
}

fn hash_bytes(bytes: &[u8], seed: u64) -> u64 {
    bytes.iter().fold(seed, |hash, byte| {
        hash.wrapping_mul(0x0000_0100_0000_01b3) ^ u64::from(*byte)
    })
}

#[derive(Clone)]
pub(crate) struct Interner {
    names: Vec<String>,
    ids: HashMap<String, NameId>,
    limit: usize,
    pub(crate) truncated: u64,
}

impl Interner {
    pub(crate) fn with_limit(limit: usize) -> Self {
        Self {
            names: Vec::new(),
            ids: HashMap::new(),
            limit,
            truncated: 0,
        }
    }

    pub(crate) fn intern(&mut self, name: &str) -> Option<NameId> {
        if name.len() <= MAX_NAME_BYTES
            && let Some(id) = self.ids.get(name)
        {
            return Some(*id);
        }
        let bounded = truncate_name(name);
        if let Some(id) = self.ids.get(&bounded) {
            return Some(*id);
        }
        if self.names.len() >= self.limit {
            self.truncated = self.truncated.saturating_add(1);
            tracing::debug!(event = "metrics_name_interner_capped");
            return None;
        }
        let id = NameId(self.names.len() as u16);
        self.names.push(bounded.clone());
        self.ids.insert(bounded, id);
        Some(id)
    }

    pub(crate) fn get(&self, id: NameId) -> &str {
        self.names
            .get(usize::from(id.0))
            .map(String::as_str)
            .unwrap_or_default()
    }

    pub(crate) fn retained_bytes(&self) -> usize {
        self.names
            .capacity()
            .saturating_mul(size_of::<String>())
            .saturating_add(self.names.iter().map(String::capacity).sum::<usize>())
            .saturating_add(
                self.ids
                    .capacity()
                    .saturating_mul(size_of::<(String, NameId)>()),
            )
            .saturating_add(self.ids.keys().map(String::capacity).sum::<usize>())
    }
}

#[derive(Clone, Default)]
pub(crate) struct SkillNameInterner {
    names: Vec<String>,
    ids: HashMap<String, NameId>,
    pub(crate) truncated: u64,
}

impl SkillNameInterner {
    pub(crate) fn intern(&mut self, name: &str) -> Option<NameId> {
        if name.len() <= MAX_SKILL_NAME_BYTES
            && let Some(id) = self.ids.get(name)
        {
            return Some(*id);
        }
        let bounded = bounded_skill_name(name);
        if let Some(id) = self.ids.get(&bounded) {
            return Some(*id);
        }
        if self.names.len() >= MAX_SKILL_NAMES {
            self.truncated = self.truncated.saturating_add(1);
            tracing::debug!(event = "metrics_skill_name_interner_capped");
            return None;
        }
        let id = NameId(self.names.len() as u16);
        self.names.push(bounded.clone());
        self.ids.insert(bounded, id);
        Some(id)
    }

    pub(crate) fn get(&self, id: NameId) -> &str {
        self.names
            .get(usize::from(id.0))
            .map(String::as_str)
            .unwrap_or_default()
    }

    pub(crate) fn retained_bytes(&self) -> usize {
        self.names
            .capacity()
            .saturating_mul(size_of::<String>())
            .saturating_add(self.names.iter().map(String::capacity).sum::<usize>())
            .saturating_add(
                self.ids
                    .capacity()
                    .saturating_mul(size_of::<(String, NameId)>()),
            )
            .saturating_add(self.ids.keys().map(String::capacity).sum::<usize>())
    }
}

pub(crate) fn bounded_skill_name(name: &str) -> String {
    if name.len() <= MAX_SKILL_NAME_BYTES {
        return name.to_string();
    }
    const PREFIX_BYTES: usize = MAX_SKILL_NAME_BYTES - 17;
    let mut end = PREFIX_BYTES;
    while !name.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}#{:016x}", &name[..end], IdentityKey::new(name).first)
}

pub(crate) fn truncate_name(name: &str) -> String {
    if name.len() <= MAX_NAME_BYTES {
        return name.to_string();
    }
    const PREFIX_BYTES: usize = MAX_NAME_BYTES - 17;
    let mut end = PREFIX_BYTES;
    while !name.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}#{:016x}", &name[..end], IdentityKey::new(name).first)
}

#[derive(Clone, Debug)]
pub(crate) struct SkillMark {
    pub(crate) ordinal: u64,
    pub(crate) tool_index: u16,
    pub(crate) name: NameId,
    pub(crate) effective_ts: i64,
    pub(crate) timestamp: Option<i64>,
    pub(crate) next_timestamp: Option<i64>,
    pub(crate) tokens_out: u64,
    pub(crate) context_tokens: u64,
}

impl SkillMark {
    pub(crate) fn observe_timestamp(&mut self, timestamp: i64) {
        let Some(start) = self.timestamp else {
            return;
        };
        if timestamp > start && self.next_timestamp.is_none_or(|next| timestamp < next) {
            self.next_timestamp = Some(timestamp);
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct LateToolCandidate {
    pub(crate) ordinal: u64,
    pub(crate) source: EventSource,
    pub(crate) provisional_builtin: bool,
    pub(crate) usage: Usage,
    pub(crate) effective_ts: i64,
    pub(crate) timestamp: Option<i64>,
    pub(crate) next_timestamp: Option<i64>,
    pub(crate) next_tool_index: u16,
    pub(crate) late_subagent_launches: u32,
    pub(crate) late_last_tool: Option<NameId>,
}

impl LateToolCandidate {
    pub(crate) fn observe_timestamp(&mut self, timestamp: i64) {
        let Some(start) = self.timestamp else {
            return;
        };
        if timestamp > start && self.next_timestamp.is_none_or(|next| timestamp < next) {
            self.next_timestamp = Some(timestamp);
        }
    }
}

pub(crate) fn add_usage(target: &mut crate::pricing::ModelTokens, usage: Usage) {
    target.input_tokens = target.input_tokens.saturating_add(usage.input_tokens);
    target.output_tokens = target.output_tokens.saturating_add(usage.output_tokens);
    target.cache_read_tokens = target
        .cache_read_tokens
        .saturating_add(usage.cache_read_tokens);
    target.cache_creation_tokens = target
        .cache_creation_tokens
        .saturating_add(usage.cache_creation_tokens);
}

pub(crate) fn add_model_tokens(
    target: &mut crate::pricing::ModelTokens,
    incoming: &crate::pricing::ModelTokens,
) {
    target.input_tokens = target.input_tokens.saturating_add(incoming.input_tokens);
    target.output_tokens = target.output_tokens.saturating_add(incoming.output_tokens);
    target.cache_read_tokens = target
        .cache_read_tokens
        .saturating_add(incoming.cache_read_tokens);
    target.cache_creation_tokens = target
        .cache_creation_tokens
        .saturating_add(incoming.cache_creation_tokens);
    target.cache_creation_1h_tokens = target
        .cache_creation_1h_tokens
        .saturating_add(incoming.cache_creation_1h_tokens);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_names_with_one_prefix_keep_distinct_ids() {
        let prefix = "model-prefix".repeat(10);
        let mut interner = Interner::with_limit(MAX_TOOL_NAMES);
        let first = interner.intern(&format!("{prefix}-a")).expect("first name");
        let second = interner
            .intern(&format!("{prefix}-b"))
            .expect("second name");
        assert_ne!(first, second);
        assert_ne!(interner.get(first), interner.get(second));
        assert!(interner.get(first).len() <= MAX_NAME_BYTES);
    }

    #[test]
    fn long_skill_names_with_one_prefix_keep_distinct_ids() {
        let prefix = "skill-prefix".repeat(20);
        let mut interner = SkillNameInterner::default();
        let first = interner.intern(&format!("{prefix}-a")).expect("first name");
        let second = interner
            .intern(&format!("{prefix}-b"))
            .expect("second name");
        assert_ne!(first, second);
        assert_ne!(interner.get(first), interner.get(second));
        assert!(interner.get(first).len() <= MAX_SKILL_NAME_BYTES);
    }

    #[test]
    fn identity_keys_use_the_complete_value() {
        let prefix = "message-prefix".repeat(10);
        assert_ne!(
            IdentityKey::new(&format!("{prefix}-a")),
            IdentityKey::new(&format!("{prefix}-b"))
        );
    }
}
