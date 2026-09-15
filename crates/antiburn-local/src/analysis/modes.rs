//! Attribute each assistant turn's tokens to the mode of work it paid for.
//!
//! The HUD token map asks "what is my agent doing right now?". This pass
//! answers it per turn: it reads the normalized stream and emits one
//! [`ModeSample`] per (turn, mode) pair. A turn that calls tools in several
//! categories splits its tokens evenly across them.

use serde::{Deserialize, Serialize};

use super::model::{EventSource, NormalizedEvent, Role, ToolCategory, Usage};

/// The kind of work one assistant turn did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WorkMode {
    /// Read or search the codebase.
    Looking,
    /// Run a shell command or a test runner.
    Running,
    /// Edit files.
    Changing,
    /// Spawn a sub-agent, or a turn inside a sub-agent transcript.
    Delegating,
    /// Extended thinking with no tool call.
    Thinking,
    /// Plain assistant text with no tool call and no thinking.
    Talking,
    /// Tools the engine does not classify: MCP, skills, web fetch.
    Other,
}

impl WorkMode {
    /// Every mode, in the order the HUD lays dots out.
    pub const ALL: [WorkMode; 7] = [
        WorkMode::Looking,
        WorkMode::Running,
        WorkMode::Changing,
        WorkMode::Delegating,
        WorkMode::Thinking,
        WorkMode::Talking,
        WorkMode::Other,
    ];

    fn from_category(category: ToolCategory) -> WorkMode {
        match category {
            ToolCategory::Read | ToolCategory::Search => WorkMode::Looking,
            ToolCategory::Bash | ToolCategory::Test => WorkMode::Running,
            ToolCategory::Edit => WorkMode::Changing,
            ToolCategory::Other => WorkMode::Other,
        }
    }
}

/// The tokens one assistant turn paid for one mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModeSample {
    /// Unix epoch milliseconds of the turn, when the transcript has one.
    pub ts_ms: Option<i64>,
    pub mode: WorkMode,
    /// Tokens the turn added: fresh input, cache writes, and output. Cache
    /// reads are excluded, the same rule as `SessionMetrics::tokens_in`.
    pub tokens: u64,
    pub source: EventSource,
}

/// True when a tool name spawns a sub-agent.
fn is_delegation_tool(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "task" | "agent" | "spawn_agent" | "subagent" | "dispatch_agent"
    )
}

fn added_tokens(usage: &Usage) -> u64 {
    usage
        .effective_input_tokens()
        .saturating_add(usage.output_tokens)
}

/// The distinct modes one turn touched, in [`WorkMode::ALL`] order.
fn turn_modes(event: &NormalizedEvent) -> Vec<WorkMode> {
    let mut modes: Vec<WorkMode> = Vec::new();
    for tool in &event.tools {
        let mode = if is_delegation_tool(&tool.name) {
            WorkMode::Delegating
        } else {
            WorkMode::from_category(tool.category)
        };
        if !modes.contains(&mode) {
            modes.push(mode);
        }
    }
    if modes.is_empty() {
        modes.push(if event.has_thinking {
            WorkMode::Thinking
        } else {
            WorkMode::Talking
        });
    }
    modes.sort_by_key(|mode| WorkMode::ALL.iter().position(|m| m == mode));
    modes
}

/// Emit one sample per (assistant turn, mode). Turns with no tokens are
/// skipped. Tokens split evenly across a turn's modes; the remainder goes to
/// the first mode so the sum stays exact.
pub fn mode_samples(events: &[NormalizedEvent]) -> Vec<ModeSample> {
    let mut samples = Vec::new();
    for event in events {
        if event.role != Role::Assistant {
            continue;
        }
        let tokens = added_tokens(&event.usage);
        if tokens == 0 {
            continue;
        }
        let modes = turn_modes(event);
        let count = modes.len() as u64;
        let share = tokens / count;
        let remainder = tokens % count;
        for (index, mode) in modes.into_iter().enumerate() {
            let extra = if index == 0 { remainder } else { 0 };
            samples.push(ModeSample {
                ts_ms: event.ts_ms,
                mode,
                tokens: share + extra,
                source: event.source,
            });
        }
    }
    samples
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::model::ToolCall;

    fn turn(tools: &[&str], thinking: bool, tokens: u64) -> NormalizedEvent {
        let mut event = NormalizedEvent::new(Role::Assistant);
        event.ts_ms = Some(1_000);
        event.has_thinking = thinking;
        event.tools = tools.iter().map(|name| ToolCall::new(*name)).collect();
        event.usage = Usage {
            input_tokens: tokens / 2,
            output_tokens: tokens - tokens / 2,
            cache_read_tokens: 90_000,
            cache_creation_tokens: 0,
            cache_creation_1h_tokens: 0,
        };
        event
    }

    fn modes_of(samples: &[ModeSample]) -> Vec<(WorkMode, u64)> {
        samples.iter().map(|s| (s.mode, s.tokens)).collect()
    }

    #[test]
    fn a_read_turn_is_looking_and_excludes_cache_reads() {
        let samples = mode_samples(&[turn(&["Read"], false, 400)]);
        assert_eq!(modes_of(&samples), vec![(WorkMode::Looking, 400)]);
        assert_eq!(samples[0].ts_ms, Some(1_000));
        assert_eq!(samples[0].source, EventSource::Parent);
    }

    #[test]
    fn a_turn_with_no_tools_is_thinking_or_talking() {
        let samples = mode_samples(&[turn(&[], true, 10), turn(&[], false, 20)]);
        assert_eq!(
            modes_of(&samples),
            vec![(WorkMode::Thinking, 10), (WorkMode::Talking, 20)]
        );
    }

    #[test]
    fn a_mixed_turn_splits_tokens_evenly_with_exact_sum() {
        let samples = mode_samples(&[turn(&["Grep", "Bash", "Edit"], true, 100)]);
        assert_eq!(
            modes_of(&samples),
            vec![
                (WorkMode::Looking, 34),
                (WorkMode::Running, 33),
                (WorkMode::Changing, 33)
            ]
        );
    }

    #[test]
    fn duplicate_categories_count_once() {
        let samples = mode_samples(&[turn(&["Read", "Glob", "Read"], false, 90)]);
        assert_eq!(modes_of(&samples), vec![(WorkMode::Looking, 90)]);
    }

    #[test]
    fn a_task_tool_is_delegating() {
        let samples = mode_samples(&[turn(&["Task"], false, 50)]);
        assert_eq!(modes_of(&samples), vec![(WorkMode::Delegating, 50)]);
    }

    #[test]
    fn test_and_bash_are_running_and_mcp_is_other() {
        let mut event = turn(&["mcp__slack__send"], false, 60);
        event
            .tools
            .push(ToolCall::with_command("Bash", Some("cargo test")));
        let samples = mode_samples(&[event]);
        assert_eq!(
            modes_of(&samples),
            vec![(WorkMode::Running, 30), (WorkMode::Other, 30)]
        );
    }

    #[test]
    fn non_assistant_and_empty_turns_are_skipped() {
        let mut user = turn(&["Read"], false, 40);
        user.role = Role::User;
        let empty = turn(&["Read"], false, 0);
        assert!(mode_samples(&[user, empty]).is_empty());
    }

    #[test]
    fn subagent_source_is_kept() {
        let mut event = turn(&["Edit"], false, 8);
        event.source = EventSource::Subagent;
        let samples = mode_samples(&[event]);
        assert_eq!(samples[0].source, EventSource::Subagent);
        assert_eq!(samples[0].mode, WorkMode::Changing);
    }
}
