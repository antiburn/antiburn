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
            cache_write_tokens_available: true,
            context_window: None,
            model: None,
        })
    }
}
