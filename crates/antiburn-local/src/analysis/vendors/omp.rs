//! Oh My Pi v3 JSONL adapter.
//!
//! OMP journals share Pi's version 3 header and core row kinds. A current
//! file begins with a fixed-width 256-byte `type: "title"` slot; a legacy
//! file begins with the header. This reader declares that slot as the OMP
//! prologue, admits only the characterized OMP core, and streams the rest
//! through the shared Pi-family scaffolding in
//! [`crate::analysis::vendors::pi`].
//!
//! The core is the version 3 session header, `message` rows whose role is
//! `user`, `assistant`, `toolResult`, or `bashExecution`, `model_change`,
//! `thinking_level_change`, `compaction`, and `custom` rows whose
//! `customType` is characterized. The shared handler reads a `custom` row as
//! inert only when it has no shared parser signal. `title_change`,
//! `credential_pin`, `ttsr_injection`, and `session_init` are OMP
//! housekeeping rows: they keep their thread link and carry no analysis
//! signal. The reader takes the skill listing in a `session_init` system
//! prompt as the session's startup context, and counts a `read` call of a
//! `skill://<name>` path as a use of that skill. Every other OMP row kind,
//! and every other header version, fails closed as an unrecognized type. Pi
//! accepts more than this, and OMP must not inherit that reach.

use serde_json::Value;

use crate::analysis::interface::{
    RecordSink, ResumedVisit, SessionCollector, SessionInput, SessionReader, VisitOutcome,
};
use crate::analysis::resume::StreamSnapshot;
use crate::analysis::source_validity::{AppendOnlyGuarantee, SourceClaim};
use crate::analysis::vendors::pi::{DialectRow, PiDialect, PiSessionReader};
use crate::analysis::{SourceCapabilities, SourceFormat};

/// The OMP journal contract: a fixed-width title slot, then the v3 core.
const OMP: PiDialect = PiDialect::new("omp", is_title_slot, classify_omp_row, true);

/// The physical width of the title slot, including its line terminator.
const TITLE_SLOT_BYTES: usize = 256;

pub struct OmpSessionReader;

impl SessionReader for OmpSessionReader {
    fn agent(&self) -> &'static str {
        "omp"
    }

    fn capabilities(&self, input: &SessionInput) -> SourceCapabilities {
        if input.source_format != SourceFormat::OmpV3Jsonl
            && input.source_format != SourceFormat::Uncharacterized
        {
            return SourceCapabilities::uncharacterized(input.source_format);
        }
        SourceCapabilities::omp()
    }

    fn normalize(
        &self,
        input: &SessionInput,
    ) -> anyhow::Result<crate::analysis::NormalizedSession> {
        let mut collector = SessionCollector::new(input.agent.clone(), input.session_id.clone());
        self.visit(input, &mut collector)?;
        collector.into_session()
    }

    fn visit(
        &self,
        input: &SessionInput,
        sink: &mut dyn RecordSink,
    ) -> anyhow::Result<VisitOutcome> {
        PiSessionReader.visit_dialect(input, sink, OMP)
    }

    fn visit_claimed(
        &self,
        input: &SessionInput,
        claim: &SourceClaim,
        guarantee: AppendOnlyGuarantee,
        cancel: &dyn Fn() -> bool,
        sink: &mut dyn RecordSink,
    ) -> anyhow::Result<VisitOutcome> {
        PiSessionReader.visit_claimed_dialect(input, claim, guarantee, cancel, sink, OMP)
    }

    fn visit_claimed_resumed(
        &self,
        input: &SessionInput,
        claim: &SourceClaim,
        resume: &StreamSnapshot,
        cancel: &dyn Fn() -> bool,
        sink: &mut dyn RecordSink,
    ) -> anyhow::Result<ResumedVisit> {
        PiSessionReader.visit_claimed_resumed_dialect(input, claim, resume, cancel, sink, OMP)
    }

    fn empty_resume_state(&self) -> Option<crate::analysis::resume::AdapterSnapshot> {
        Some(PiSessionReader::empty_adapter_snapshot())
    }
}

/// Tells if `bytes` is the fixed-width title slot.
///
/// The slot is padded to an exact width, so a `title` record of another
/// length is not the slot. It then fails closed as an unrecognized type
/// instead of being dropped silently.
fn is_title_slot(bytes: &[u8], value: &Value) -> bool {
    // The framed record excludes its line terminator.
    bytes.len() + 1 == TITLE_SLOT_BYTES
        && value.get("type").and_then(Value::as_str) == Some("title")
}

/// Places `value` in the characterized OMP contract.
fn classify_omp_row(value: &Value) -> DialectRow {
    let shared = match value.get("type").and_then(Value::as_str) {
        // OMP writes version 3. Older headers have their own migrations,
        // which no fixture pins yet.
        Some("session") => value.get("version").and_then(Value::as_u64) == Some(3),
        Some("message") => matches!(
            value
                .get("message")
                .and_then(|message| message.get("role"))
                .and_then(Value::as_str),
            Some("user" | "assistant" | "toolResult" | "bashExecution")
        ),
        Some("model_change" | "thinking_level_change" | "compaction") => true,
        // Tool start markers, exit markers, todo state, and goal summaries.
        // Usage and tool facts come from the message rows, not from these.
        Some("custom") => matches!(
            value.get("customType").and_then(Value::as_str),
            Some(
                "tool_execution_start"
                    | "session_exit"
                    | "todo_hud_state"
                    | "user_todo_edit"
                    | "goal-completed"
            )
        ),
        Some("title_change" | "credential_pin" | "ttsr_injection" | "session_init") => {
            return DialectRow::Housekeeping;
        }
        _ => false,
    };
    if shared {
        DialectRow::Shared
    } else {
        DialectRow::Outside
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::interface::{RawSource, SessionCollector};
    use crate::analysis::{PartialReason, SourceFormat};

    fn title_slot(title: &str) -> String {
        let mut line = format!(
            r#"{{"type":"title","v":1,"title":"{title}","source":"auto","updatedAt":"2026-01-01T00:00:00.000Z","pad":""}}"#
        );
        while line.len() < 255 {
            line.insert(line.len() - 2, ' ');
        }
        line.push('\n');
        assert_eq!(line.len(), 256);
        line
    }

    fn input(content: &str) -> SessionInput {
        SessionInput {
            agent: "omp".to_owned(),
            session_id: "synthetic".to_owned(),
            source: RawSource::Jsonl(content.to_owned()),
            source_format: SourceFormat::OmpV3Jsonl,
            fork_parent_session_id: None,
        }
    }

    fn core_session() -> String {
        let mut body = title_slot("synthetic");
        body.push_str(
            r#"{"type":"session","version":3,"id":"s1","timestamp":"2026-01-01T00:00:00.000Z","cwd":"/tmp/synthetic"}
{"type":"message","id":"m1","parentId":null,"timestamp":"2026-01-01T00:00:01.000Z","message":{"role":"user","content":[{"type":"text","text":"hi"}]}}
{"type":"thinking_level_change","id":"t1","parentId":"m1","timestamp":"2026-01-01T00:00:01.500Z","thinkingLevel":"high"}
{"type":"message","id":"m2","parentId":"t1","timestamp":"2026-01-01T00:00:02.000Z","message":{"role":"assistant","provider":"anthropic","api":"messages","model":"claude-opus-4-6","timestamp":1,"usage":{"input":10,"output":4,"cacheRead":0,"cacheWrite":0},"content":[{"type":"text","text":"ok"}]}}
"#,
        );
        body
    }

    #[test]
    fn title_slot_then_v3_header_normalizes() {
        let session = OmpSessionReader.normalize(&input(&core_session())).unwrap();
        assert!(!session.events.is_empty());
        assert!(
            session
                .events
                .iter()
                .any(|event| event.model.as_deref() == Some("claude-opus-4-6"))
        );
    }

    #[test]
    fn unknown_omp_type_is_unrecognized() {
        let mut body = core_session();
        body.push_str(
            r#"{"type":"reset_boundary","id":"r1","parentId":"m2","timestamp":"2026-01-01T00:00:03.000Z"}
"#,
        );
        let mut sink = SessionCollector::new("omp", "synthetic");
        OmpSessionReader.visit(&input(&body), &mut sink).unwrap();
        assert!(
            sink.partial_reasons()
                .contains(&PartialReason::UnrecognizedRecordType)
        );
    }

    fn unrecognized_reasons(body: &str) -> std::collections::BTreeSet<PartialReason> {
        let mut sink = SessionCollector::new("omp", "synthetic");
        OmpSessionReader.visit(&input(body), &mut sink).unwrap();
        sink.partial_reasons().clone()
    }

    /// The baseline the rejection tests measure against: the core alone
    /// admits cleanly, so each rejection below comes from the added row.
    #[test]
    fn the_characterized_core_admits_without_an_unrecognized_row() {
        assert!(
            !unrecognized_reasons(&core_session()).contains(&PartialReason::UnrecognizedRecordType)
        );
    }

    /// A `title` record of the wrong width is not the fixed-width slot, so
    /// it must fail closed instead of being dropped as the prologue.
    #[test]
    fn a_title_record_outside_the_fixed_slot_width_is_unrecognized() {
        let mut body = r#"{"type":"title","title":"short"}"#.to_owned();
        body.push('\n');
        body.push_str(&core_session()[title_slot("synthetic").len()..]);
        assert!(
            unrecognized_reasons(&body).contains(&PartialReason::UnrecognizedRecordType),
            "a short title record must not pass as the slot"
        );
    }

    /// Pi accepts header versions 1 and 2 through its documented read-time
    /// migrations. No fixture pins the OMP equivalents, so OMP takes 3 only.
    #[test]
    fn a_pre_v3_header_is_unrecognized() {
        let body = core_session().replace(r#""version":3"#, r#""version":2"#);
        assert!(unrecognized_reasons(&body).contains(&PartialReason::UnrecognizedRecordType));
    }

    /// These rows are valid Pi input. They are outside the characterized OMP
    /// core, so the OMP dialect must not inherit Pi's acceptance.
    #[test]
    fn pi_rows_outside_the_omp_core_are_unrecognized() {
        for row in [
            r#"{"type":"usage","id":"u1","parentId":"m2","timestamp":"2026-01-01T00:00:03.000Z","usage":{"input":1,"output":1,"cacheRead":0,"cacheWrite":0}}"#,
            r#"{"type":"branch_summary","id":"b1","parentId":"m2","fromId":"m1","timestamp":"2026-01-01T00:00:03.000Z","summary":"s"}"#,
            r#"{"type":"label","id":"l1","parentId":"m2","timestamp":"2026-01-01T00:00:03.000Z","label":"x"}"#,
            r#"{"type":"message","id":"p1","parentId":"m2","timestamp":"2026-01-01T00:00:03.000Z","message":{"role":"pythonExecution","content":[]}}"#,
        ] {
            let mut body = core_session();
            body.push_str(row);
            body.push('\n');
            assert!(
                unrecognized_reasons(&body).contains(&PartialReason::UnrecognizedRecordType),
                "{row}"
            );
        }
    }

    /// Real OMP journals interleave these rows with the core. They carry no
    /// analysis signal, so a session that holds them stays complete.
    #[test]
    fn characterized_housekeeping_rows_keep_the_session_complete() {
        let mut body = core_session();
        body.push_str(
            r#"{"type":"title_change","id":"h1","parentId":"m2","timestamp":"2026-01-01T00:00:03.000Z","title":"t","previousTitle":"s","source":"auto","trigger":"auto"}
{"type":"credential_pin","id":"h2","parentId":"h1","timestamp":"2026-01-01T00:00:03.100Z","provider":"anthropic","hash":"abc"}
{"type":"custom","customType":"tool_execution_start","data":{"toolCallId":"c1","toolName":"bash","startedAt":"2026-01-01T00:00:03.200Z","args":{"command":"ls"}},"id":"h3","parentId":"h2","timestamp":"2026-01-01T00:00:03.200Z"}
{"type":"ttsr_injection","id":"h4","parentId":"h3","timestamp":"2026-01-01T00:00:03.300Z","injectedRules":["r"]}
{"type":"custom","customType":"session_exit","data":{"reason":"dispose","kind":"normal"},"id":"h5","parentId":"h4","timestamp":"2026-01-01T00:00:03.400Z"}
"#,
        );
        assert!(unrecognized_reasons(&body).is_empty());
    }

    /// Housekeeping admits a row kind only in its inert shape, and an
    /// uncharacterized `customType` stays outside the contract.
    #[test]
    fn housekeeping_with_a_signal_or_an_unknown_custom_type_is_unrecognized() {
        for row in [
            r#"{"type":"title_change","id":"h1","parentId":"m2","timestamp":"2026-01-01T00:00:03.000Z","title":"t","usage":{"input":1,"output":1}}"#,
            r#"{"type":"custom","customType":"tool_execution_start","id":"h1","parentId":"m2","timestamp":"2026-01-01T00:00:03.000Z","model":"x","data":{}}"#,
            r#"{"type":"custom","customType":"not_characterized","id":"h1","parentId":"m2","timestamp":"2026-01-01T00:00:03.000Z","data":{}}"#,
        ] {
            let mut body = core_session();
            body.push_str(row);
            body.push('\n');
            assert!(
                unrecognized_reasons(&body).contains(&PartialReason::UnrecognizedRecordType),
                "{row}"
            );
        }
    }

    fn session_init_row() -> &'static str {
        r#"{"type":"session_init","id":"i1","parentId":"m2","timestamp":"2026-01-01T00:00:03.000Z","systemPrompt":"Intro.\n<skills>\n- deep-research: Research harness.\n- unused-skill: Never read.\n</skills>\nOutro.","task":"t","tools":["read","bash"],"agent":"task","resolvedModel":"anthropic/claude-opus-4-6","readOnly":false,"spawns":"*"}"#
    }

    fn skill_read_row() -> &'static str {
        r#"{"type":"message","id":"m3","parentId":"i1","timestamp":"2026-01-01T00:00:04.000Z","message":{"role":"assistant","provider":"anthropic","api":"messages","model":"claude-opus-4-6","timestamp":4,"usage":{"input":10,"output":4,"cacheRead":0,"cacheWrite":0},"content":[{"type":"toolCall","id":"c1","name":"read","arguments":{"path":"skill://deep-research/references/a.md"}},{"type":"toolCall","id":"c2","name":"read","arguments":{"path":"src/lib.rs"}},{"type":"toolCall","id":"c3","name":"read","arguments":{"path":"skill://team_research"}}]}}"#
    }

    fn stream_summary(body: &str) -> crate::analysis::interface::SessionSummary {
        let mut sink = SessionCollector::new("omp", "synthetic");
        PiSessionReader
            .visit_reader_dialect(
                std::io::BufReader::new(body.as_bytes()),
                &|| false,
                &mut sink,
                crate::analysis::vendors::pi::PiStreamState::default(),
                OMP,
            )
            .unwrap()
            .finish()
    }

    /// A subagent journal records its system prompt in `session_init`. Its
    /// skill listing is the startup context, and the stream and batch
    /// passes agree on it.
    #[test]
    fn session_init_skill_listing_is_the_startup_context() {
        let body = format!("{}{}\n", core_session(), session_init_row());
        assert!(unrecognized_reasons(&body).is_empty());

        let summary = stream_summary(&body);
        let context = summary.initial_context.expect("startup context");
        let skills: Vec<_> = context
            .sources
            .iter()
            .map(|source| (source.source.as_str(), source.source_name.as_deref()))
            .collect();
        assert_eq!(
            skills,
            [
                ("skill_instructions", Some("deep-research")),
                ("skill_instructions", Some("unused-skill")),
            ]
        );
        assert_eq!(
            summary
                .skill_descriptions
                .get("deep-research")
                .map(String::as_str),
            Some("Research harness.")
        );
        assert_eq!(
            crate::analysis::initial_context::parse_initial_context("omp", &body),
            Some(context)
        );
    }

    /// A top-level journal has no `session_init` row, so its startup context
    /// is unavailable, not empty.
    #[test]
    fn a_journal_without_session_init_has_no_startup_context() {
        assert!(stream_summary(&core_session()).initial_context.is_none());
        assert!(
            crate::analysis::initial_context::parse_initial_context("omp", &core_session())
                .is_none()
        );
    }

    /// OMP loads a skill with a `read` of its `skill://` URI. That call is a
    /// skill use; a `read` of an ordinary path stays a `read`.
    #[test]
    fn a_skill_uri_read_is_a_skill_use() {
        let body = format!(
            "{}{}\n{}\n",
            core_session(),
            session_init_row(),
            skill_read_row()
        );
        let session = OmpSessionReader.normalize(&input(&body)).unwrap();
        let tools: Vec<_> = session
            .events
            .iter()
            .flat_map(|event| &event.tools)
            .map(|tool| (tool.name.as_str(), tool.detail.as_deref()))
            .collect();
        // OMP matches the URI name exactly, so a name outside Pi's name
        // rules is still a skill use.
        assert_eq!(
            tools,
            [
                ("skill", Some("deep-research")),
                ("read", None),
                ("skill", Some("team_research")),
            ]
        );
    }

    #[test]
    fn capabilities_are_omp_not_pi() {
        let caps = OmpSessionReader.capabilities(&input("{}"));
        assert_eq!(caps.source_format, SourceFormat::OmpV3Jsonl);
        assert!(caps.model_identity);
        assert!(caps.reasoning_effort_tier);
        assert!(!caps.subagent_relationships);
    }
}
