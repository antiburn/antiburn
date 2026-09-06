//! Generic JSONL fallback adapter.
//!
//! Used for any vendor without a bespoke adapter. The shared record parser
//! already understands both the Anthropic and OpenAI transcript shapes, which
//! covers the great majority of JSONL-emitting agents (Amp, Cline, Copilot,
//! Windsurf, …). Vendors that diverge get a dedicated adapter later without
//! the engine ever changing.

use anyhow::Context;

use super::read_source;
use crate::analysis::interface::{SessionInput, VendorAdapter};
use crate::analysis::model::NormalizedSession;
use crate::analysis::records::parse_jsonl;

pub struct GenericJsonlAdapter;

impl VendorAdapter for GenericJsonlAdapter {
    fn agent(&self) -> &'static str {
        "generic"
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
            model: None,
        })
    }
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

        let session = GenericJsonlAdapter
            .normalize(&input)
            .expect("empty session normalizes");

        assert!(!session.cache_write_tokens_available);
    }
}
