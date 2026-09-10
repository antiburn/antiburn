//! Generic JSONL fallback adapter.
//!
//! Used for any vendor without a bespoke adapter. The shared record parser
//! already understands both the Anthropic and OpenAI transcript shapes, which
//! covers the great majority of JSONL-emitting agents (Amp, Cline, Copilot,
//! Windsurf, …). Vendors that diverge get a dedicated adapter later without
//! the engine ever changing.

use std::io::BufReader;

use anyhow::Context;

use super::read_source;
use crate::analysis::framing::{BoundedJsonlReader, FramedRecord, RecordSkip};
use crate::analysis::interface::{
    ContextWindowSource, NormalizedRecord, RawSource, RecordSink, SessionInput, SessionReader,
    SessionSummary, VisitOutcome,
};
use crate::analysis::model::NormalizedSession;
use crate::analysis::records::parse_jsonl;
use crate::analysis::source_validity::{AppendOnlyGuarantee, PinnedSource, SourceClaim};

pub struct GenericJsonlSessionReader;

impl SessionReader for GenericJsonlSessionReader {
    fn agent(&self) -> &'static str {
        "generic"
    }

    fn capabilities(
        &self,
        _source: &crate::analysis::RawSource,
    ) -> crate::analysis::SourceCapabilities {
        crate::analysis::SourceCapabilities::generic()
    }

    fn normalize(&self, input: &SessionInput) -> anyhow::Result<NormalizedSession> {
        let content = read_source(&input.source)
            .with_context(|| format!("reading session {}", input.session_id))?;
        Ok(NormalizedSession {
            agent: input.agent.clone(),
            session_id: input.session_id.clone(),
            events: parse_jsonl(&content),
            // An unknown vendor's cache-write support is not a fact this
            // adapter can verify. A dedicated adapter (for example
            // `SourceCapabilities::claude`) can claim it because it knows its
            // vendor's transcript contract; this fallback knows no vendor.
            cache_write_tokens_available: false,
            context_window: None,
            context_window_source: ContextWindowSource::Inferred,
            model: None,
        })
    }

    fn visit_claimed(
        &self,
        input: &SessionInput,
        claim: &SourceClaim,
        guarantee: AppendOnlyGuarantee,
        cancel: &dyn Fn() -> bool,
        sink: &mut dyn RecordSink,
    ) -> anyhow::Result<VisitOutcome> {
        let RawSource::File(path) = &input.source else {
            anyhow::bail!("a claimed generic source must be a file");
        };
        let mut pinned = match PinnedSource::open(path, claim.clone())? {
            Ok(pinned) => pinned,
            Err(reason) => return Ok(VisitOutcome::SourceChanged(reason)),
        };
        let limit = match guarantee {
            AppendOnlyGuarantee::Evidenced => claim.boundary,
            AppendOnlyGuarantee::Absent => u64::MAX,
        };
        visit_reader(BufReader::new(pinned.reader(limit)), cancel, sink)?;
        let outcome = match guarantee {
            AppendOnlyGuarantee::Evidenced => pinned.recheck_prefix()?.map_or(
                VisitOutcome::AcceptedPrefix {
                    boundary: claim.boundary,
                },
                VisitOutcome::SourceChanged,
            ),
            AppendOnlyGuarantee::Absent => pinned
                .recheck_full()?
                .map_or(VisitOutcome::AcceptedFull, VisitOutcome::SourceChanged),
        };
        if !matches!(outcome, VisitOutcome::SourceChanged(_)) {
            sink.finish(SessionSummary::default());
        }
        Ok(outcome)
    }
}

fn visit_reader(
    reader: impl std::io::BufRead,
    cancel: &dyn Fn() -> bool,
    sink: &mut dyn RecordSink,
) -> anyhow::Result<()> {
    let mut reader = BoundedJsonlReader::new(reader);
    while let Some(record) = reader.next_record(cancel) {
        match record {
            FramedRecord::Complete { bytes, .. } => {
                let record = std::str::from_utf8(bytes).context("generic record is not UTF-8")?;
                for event in parse_jsonl(record) {
                    sink.record(NormalizedRecord::MetricsEvent(Box::new(event)));
                }
            }
            FramedRecord::Skipped(RecordSkip::ReadFailed { index, kind }) => {
                anyhow::bail!("generic record {index} read failed: {kind:?}");
            }
            FramedRecord::Skipped(RecordSkip::Cancelled { index }) => {
                anyhow::bail!("generic record {index} read was cancelled");
            }
            FramedRecord::Skipped(skip) => {
                sink.record(NormalizedRecord::Unusable(skip.partial_reason()));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::interface::RawSource;

    /// Freezes the generic fallback's minimum capability claim (plan decision
    /// 7): an unknown vendor's transcript proves no vendor-specific contract,
    /// so this adapter never claims cache-write support.
    #[test]
    fn generic_fallback_never_claims_cache_write_support() {
        let input = SessionInput {
            agent: "generic".to_owned(),
            session_id: "generic-session".to_owned(),
            source: RawSource::Jsonl(String::new()),
            fork_parent_session_id: None,
        };

        let session = GenericJsonlSessionReader
            .normalize(&input)
            .expect("empty session normalizes");

        assert!(!session.cache_write_tokens_available);
    }
}
