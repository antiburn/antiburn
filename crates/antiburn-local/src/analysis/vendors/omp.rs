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
//! `thinking_level_change`, and `compaction`. Every other OMP row kind, and
//! every other header version, fails closed as an unrecognized type. Pi
//! accepts more than this, and OMP must not inherit that reach.

use serde_json::Value;

use crate::analysis::interface::{
    RecordSink, ResumedVisit, SessionCollector, SessionInput, SessionReader, VisitOutcome,
};
use crate::analysis::resume::StreamSnapshot;
use crate::analysis::source_validity::{AppendOnlyGuarantee, SourceClaim};
use crate::analysis::vendors::pi::{PiDialect, PiSessionReader};
use crate::analysis::{SourceCapabilities, SourceFormat};

/// The OMP journal contract: a fixed-width title slot, then the v3 core.
const OMP: PiDialect = PiDialect::new("omp", is_title_slot, is_omp_core);

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

/// Tells if `value` is inside the characterized OMP core.
fn is_omp_core(value: &Value) -> bool {
    match value.get("type").and_then(Value::as_str) {
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
        _ => false,
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

    #[test]
    fn capabilities_are_omp_not_pi() {
        let caps = OmpSessionReader.capabilities(&input("{}"));
        assert_eq!(caps.source_format, SourceFormat::OmpV3Jsonl);
        assert!(caps.model_identity);
        assert!(caps.reasoning_effort_tier);
        assert!(!caps.subagent_relationships);
    }
}
