//! The built-in tool catalogue.
//!
//! This holds the measured token cost of each built-in tool (`Bash`, `Read`,
//! …) for each harness version and model. `scripts/build-tool-catalog.mjs`
//! builds the file at release time from a capture of real tool definitions;
//! the crate embeds it as a plain string and parses it once.
//!
//! A harness ships many versions, and a version's tool set can differ across
//! models (a beta tool may reach only one model family). [`ToolCatalog::lookup`]
//! resolves both: the nearest version at or below the one the session ran, and
//! the closest matching model.

use std::collections::HashMap;
use std::sync::OnceLock;

use serde::Deserialize;

/// One built-in tool's measured cost, resolved for one harness version and model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogTool {
    /// The tool's canonical catalogue name (lowercase, e.g. `bash`).
    pub name: String,
    /// Names the harness itself uses for this tool (e.g. `Bash`), in capture
    /// order. Empty when the capture recorded no alias.
    pub aliases: Vec<String>,
    /// The tool definition's measured token cost for the resolved model.
    pub tokens: u32,
}

impl CatalogTool {
    /// Every name a harness could call this tool by: its canonical
    /// catalogue name, each alias, and each alias's last dot-separated
    /// segment (a namespaced alias such as Codex's `functions.exec` is
    /// never the name a session calls the tool by; the transcript uses
    /// its bare last segment). Mirrors `initial_context::builtin_tool_rows`'s
    /// prior `match_names` construction exactly.
    pub fn match_names(&self) -> Vec<String> {
        let mut names = vec![self.name.clone()];
        for alias in &self.aliases {
            names.push(alias.clone());
            names.push(last_dot_segment(alias).to_string());
        }
        names
    }

    /// The name to display and key this tool by: its first alias when it
    /// has one, else its canonical name, reduced to the last dot
    /// segment. Mirrors `initial_context::builtin_tool_rows`'s prior
    /// `source_name` construction exactly.
    pub fn display_name(&self) -> String {
        let raw_name = self
            .aliases
            .first()
            .map_or(self.name.as_str(), String::as_str);
        last_dot_segment(raw_name).to_string()
    }

    /// True when `deferred` names this tool by its canonical name or any
    /// alias (case-insensitive). A match name's last-segment reduction is
    /// never checked here — a harness reports a deferral under the tool's
    /// own spelling. Mirrors `initial_context::builtin_tool_rows`'s prior
    /// deferred check exactly. Generic over the caller's set type
    /// (`HashSet` on the metrics side, `BTreeSet` on the evidence side).
    pub fn is_deferred<'a>(&self, deferred: impl IntoIterator<Item = &'a String>) -> bool {
        deferred.into_iter().any(|name| {
            name.eq_ignore_ascii_case(&self.name)
                || self
                    .aliases
                    .iter()
                    .any(|alias| name.eq_ignore_ascii_case(alias))
        })
    }
}

/// The last `.`-separated segment of a namespaced tool alias, or the whole
/// string when it carries no `.` at all (every Claude alias, e.g. `Bash`).
fn last_dot_segment(name: &str) -> &str {
    name.rsplit('.').next().unwrap_or(name)
}

/// Tool names that enter the request only when the harness actually uses
/// them, so they carry no idle context cost and can never be an honest
/// unused-built-in-tool finding. Curated policy copied verbatim from
/// Cadence's harness knowledge base (`crates/harness-kb/src/lib.rs`,
/// `situational_tools`) for the agents Cadence has reviewed. Do not add an
/// entry Cadence does not have.
pub fn situational_tools(agent: &str) -> &'static [&'static str] {
    match agent.to_ascii_lowercase().as_str() {
        "claude" => &[
            "skill",
            "tool_search",
            "ask_user_question",
            "enter_plan_mode",
            "exit_plan_mode",
            "artifact",
            "monitor",
        ],
        _ => &[],
    }
}

/// Reduces a tool name to its lowercased alphanumeric characters, matching
/// Cadence's `comparable_tool_name` (`crates/harness-kb/src/lib.rs`). Two
/// names compare equal under this reduction when they differ only by case,
/// spacing, or punctuation.
pub fn comparable_tool_name(name: &str) -> String {
    name.chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|character| character.to_ascii_lowercase())
        .collect()
}

/// The parsed catalogue file. Immutable after construction.
pub struct ToolCatalog {
    agents: HashMap<String, AgentCatalog>,
}

#[derive(Deserialize)]
struct CatalogFile {
    agents: HashMap<String, AgentCatalog>,
}

#[derive(Deserialize)]
struct AgentCatalog {
    /// Harness version string to its index into `surfaces`.
    versions: HashMap<String, usize>,
    surfaces: Vec<Surface>,
}

#[derive(Deserialize)]
struct Surface {
    tools: Vec<ToolEntry>,
}

#[derive(Deserialize)]
struct ToolEntry {
    name: String,
    #[serde(default)]
    aliases: Vec<String>,
    /// Measured token cost, keyed by model id. A model absent from this map
    /// does not carry this tool at all — the tool set can differ by model
    /// inside one harness version.
    #[serde(default)]
    tokens: HashMap<String, u32>,
}

impl ToolCatalog {
    /// Parse a catalogue file already in memory. Production code should use
    /// [`embedded`] instead; this constructor exists so a test can load a
    /// fixture catalogue instead of the real embedded one.
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        let file: CatalogFile = serde_json::from_str(json)?;
        Ok(Self {
            agents: file.agents,
        })
    }

    /// List the tools a session running `agent` at `version` with `model`
    /// carries, with each tool's measured token cost for that model.
    ///
    /// Version resolution: an exact match wins; otherwise the nearest lower
    /// version (by component-wise numeric compare); otherwise `None` when the
    /// requested version is older than every captured version.
    ///
    /// Model resolution, once a version resolves: an exact model id wins;
    /// otherwise a model in that version's surface whose family matches (see
    /// [`model_family`]); otherwise the first model id in sorted order that
    /// carries any tool in that surface. `None` only when the surface carries
    /// no model at all.
    ///
    /// Returns `None` when the agent or the version cannot resolve. Returns
    /// `Some` with only the tools present for the resolved model — a tool
    /// missing from that model's coverage in this version is left out, not
    /// zeroed.
    pub fn lookup(&self, agent: &str, version: &str, model: &str) -> Option<Vec<CatalogTool>> {
        let agent_catalog = self.agents.get(&agent.to_ascii_lowercase())?;
        let surface_index = resolve_version(&agent_catalog.versions, version)?;
        let surface = agent_catalog.surfaces.get(surface_index)?;
        let resolved_model = resolve_model(surface, model)?;
        Some(
            surface
                .tools
                .iter()
                .filter_map(|tool| {
                    tool.tokens.get(&resolved_model).map(|&tokens| CatalogTool {
                        name: tool.name.clone(),
                        aliases: tool.aliases.clone(),
                        tokens,
                    })
                })
                .collect(),
        )
    }
}

/// The catalogue embedded in the binary at build time, parsed once.
pub fn embedded() -> &'static ToolCatalog {
    static CATALOG: OnceLock<ToolCatalog> = OnceLock::new();
    CATALOG.get_or_init(|| {
        ToolCatalog::from_json(include_str!(concat!(env!("OUT_DIR"), "/tool_catalog.json")))
            .expect("the embedded tool catalog must be valid JSON")
    })
}

/// Parse a version string into numeric components (`"2.1.246"` →
/// `[2, 1, 246]`). `None` when a component is not a plain number — this
/// rejects a malformed version rather than guessing its order.
fn parse_version_components(version: &str) -> Option<Vec<u64>> {
    let parts: Vec<u64> = version
        .split('.')
        .map(str::parse::<u64>)
        .collect::<Result<_, _>>()
        .ok()?;
    (!parts.is_empty()).then_some(parts)
}

/// Resolve `target` to a surface index: an exact key match, else the nearest
/// lower version by component-wise numeric compare, else `None`.
fn resolve_version(versions: &HashMap<String, usize>, target: &str) -> Option<usize> {
    if let Some(&index) = versions.get(target) {
        return Some(index);
    }
    let target_parts = parse_version_components(target)?;
    versions
        .iter()
        .filter_map(|(version, &index)| {
            parse_version_components(version).map(|parts| (parts, index))
        })
        .filter(|(parts, _)| *parts <= target_parts)
        .max_by(|(left, _), (right, _)| left.cmp(right))
        .map(|(_, index)| index)
}

/// Resolve `target` to a model id present in `surface`: an exact id, else a
/// same-family id (see [`model_family`]), else the first id in sorted order
/// that carries any tool. `None` when the surface carries no model at all.
fn resolve_model(surface: &Surface, target: &str) -> Option<String> {
    let mut models: Vec<&str> = surface
        .tools
        .iter()
        .flat_map(|tool| tool.tokens.keys().map(String::as_str))
        .collect();
    models.sort_unstable();
    models.dedup();

    if models.binary_search(&target).is_ok() {
        return Some(target.to_string());
    }
    let target_family = model_family(target);
    if let Some(&model) = models
        .iter()
        .find(|&&model| model_family(model) == target_family)
    {
        return Some(model.to_string());
    }
    models.first().map(|model| model.to_string())
}

/// A model's family: its id with the trailing date or minor-version part
/// stripped, so a model released after the catalogue was captured can still
/// fall back to a sibling. Examples: `claude-sonnet-4-5-20250929` →
/// `claude-sonnet-4-5` (an 8-digit trailing date drops); `gpt-5.4` → `gpt-5`
/// (a trailing `<major>.<minor>` keeps only the major part); `claude-sonnet-5`
/// is unchanged (its trailing part is neither shape).
fn model_family(model: &str) -> String {
    let mut parts: Vec<String> = model.split('-').map(str::to_string).collect();
    if parts.len() > 1 {
        let last = parts.last().expect("checked len > 1").clone();
        if last.len() == 8 && last.chars().all(|c| c.is_ascii_digit()) {
            parts.pop();
            return parts.join("-");
        }
        if let Some((major, minor)) = last.split_once('.')
            && !major.is_empty()
            && !minor.is_empty()
            && major.chars().all(|c| c.is_ascii_digit())
            && minor.chars().all(|c| c.is_ascii_digit())
        {
            parts.pop();
            parts.push(major.to_string());
            return parts.join("-");
        }
    }
    parts.join("-")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> ToolCatalog {
        ToolCatalog::from_json(include_str!("../../tests/fixtures/tool_catalog.json"))
            .expect("fixture catalog must parse")
    }

    #[test]
    fn resolves_an_exact_version() {
        let catalog = fixture();
        let tools = catalog
            .lookup("claude", "2.1.246", "claude-fable-5")
            .expect("2.1.246 is an exact version");
        assert!(tools.iter().any(|tool| tool.name == "bash"));
    }

    #[test]
    fn falls_back_to_the_nearest_lower_version() {
        let catalog = fixture();
        // 2.1.240 sits strictly between the captured 2.1.232 and 2.1.233, so
        // the nearest lower version must resolve to 2.1.233, not 2.1.232.
        // `claude-fable-5` tells the two apart: it carries `task_create` at
        // 2.1.232 but loses it at 2.1.233 (the fixture's task_* narrowing).
        let at_232 = catalog
            .lookup("claude", "2.1.232", "claude-fable-5")
            .expect("2.1.232 is an exact version");
        assert!(at_232.iter().any(|tool| tool.name == "task_create"));

        let at_240 = catalog
            .lookup("claude", "2.1.240", "claude-fable-5")
            .expect("2.1.240 falls back to 2.1.233");
        assert!(!at_240.iter().any(|tool| tool.name == "task_create"));
    }

    #[test]
    fn a_version_older_than_every_capture_resolves_to_none() {
        let catalog = fixture();
        assert!(
            catalog
                .lookup("claude", "2.1.100", "claude-fable-5")
                .is_none()
        );
    }

    #[test]
    fn resolves_an_exact_model() {
        let catalog = fixture();
        let tools = catalog
            .lookup("codex", "0.149.1", "gpt-5.6-sol")
            .expect("gpt-5.6-sol is an exact model at 0.149.1");
        assert!(
            tools
                .iter()
                .any(|tool| tool.name == "collaboration_spawn_agent")
        );
    }

    #[test]
    fn falls_back_to_a_same_family_model() {
        let catalog = fixture();
        // No 2.1.246 capture carries this exact hypothetical dated id, but its
        // family (`claude-sonnet-4-5`) matches the real captured
        // `claude-sonnet-4-5-20250929`, which does carry the task_* family.
        let tools = catalog
            .lookup("claude", "2.1.246", "claude-sonnet-4-5-20991231")
            .expect("family fallback resolves to claude-sonnet-4-5-20250929");
        assert!(tools.iter().any(|tool| tool.name == "task_create"));
    }

    #[test]
    fn falls_back_to_the_first_model_when_nothing_matches() {
        let catalog = fixture();
        // "gpt-5" names no Claude model or family, so this falls all the way
        // back to the first model id in sorted order: claude-fable-5.
        let tools = catalog
            .lookup("claude", "2.1.246", "gpt-5")
            .expect("first-model fallback still resolves");
        assert!(tools.iter().any(|tool| tool.name == "bash"));
        // claude-fable-5 carries no task_* tool at 2.1.246 (see the next test).
        assert!(!tools.iter().any(|tool| tool.name == "task_create"));
    }

    #[test]
    fn a_tools_model_coverage_can_differ_within_one_version() {
        let catalog = fixture();
        let sonnet = catalog
            .lookup("claude", "2.1.246", "claude-sonnet-4-5-20250929")
            .expect("exact model");
        let fable = catalog
            .lookup("claude", "2.1.246", "claude-fable-5")
            .expect("exact model");
        assert!(sonnet.iter().any(|tool| tool.name == "task_create"));
        assert!(!fable.iter().any(|tool| tool.name == "task_create"));
    }

    #[test]
    fn an_unknown_agent_resolves_to_none() {
        let catalog = fixture();
        assert!(
            catalog
                .lookup("cursor", "1.0.0", "claude-fable-5")
                .is_none()
        );
    }
}
