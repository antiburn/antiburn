//! Dedicated fail-closed readers for formats without detector-grade contracts.

use super::generic_jsonl::GenericJsonlSessionReader;
use crate::analysis::interface::{RecordSink, SessionInput, SessionReader, VisitOutcome};
use crate::analysis::model::NormalizedSession;
use crate::analysis::source_validity::{AppendOnlyGuarantee, SourceClaim};
use crate::analysis::{SourceCapabilities, SourceFormat};

pub struct PassiveSessionReader {
    pub agent: &'static str,
    pub format: SourceFormat,
}

impl SessionReader for PassiveSessionReader {
    fn agent(&self) -> &'static str {
        self.agent
    }

    fn capabilities(&self, input: &SessionInput) -> SourceCapabilities {
        SourceCapabilities::uncharacterized(input.source_format_or(self.format))
    }

    fn normalize(&self, input: &SessionInput) -> anyhow::Result<NormalizedSession> {
        GenericJsonlSessionReader.normalize(input)
    }

    fn visit_claimed(
        &self,
        input: &SessionInput,
        claim: &SourceClaim,
        guarantee: AppendOnlyGuarantee,
        cancel: &dyn Fn() -> bool,
        sink: &mut dyn RecordSink,
    ) -> anyhow::Result<VisitOutcome> {
        GenericJsonlSessionReader.visit_claimed(input, claim, guarantee, cancel, sink)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_variants_keep_fail_closed_capabilities() {
        for (agent, path, format) in [
            ("copilot", "chat.json", SourceFormat::CopilotIdeChatJson),
            ("kiro", "session.json", SourceFormat::KiroSessionJson),
            ("kiro", "session.chat", SourceFormat::KiroChat),
            ("kiro", "session.v3", SourceFormat::KiroCliV3Bundle),
            ("kiro", "chat-save.json", SourceFormat::KiroChatSaveExport),
            ("amp-code", "thread.json", SourceFormat::AmpThreadJson),
            (
                "amp-code",
                "file-changes.json",
                SourceFormat::AmpFileChanges,
            ),
            (
                "windsurf",
                "workspace.json",
                SourceFormat::WindsurfWorkspaceJson,
            ),
            (
                "windsurf",
                "cascade.pb",
                SourceFormat::WindsurfCascadeProtobuf,
            ),
            ("windsurf", "mirror.json", SourceFormat::WindsurfMirrorJson),
        ] {
            let reader = super::super::reader_for(agent);
            let input = SessionInput {
                agent: agent.to_owned(),
                session_id: "test".to_owned(),
                source: crate::analysis::RawSource::File(path.into()),
                source_format: format,
                fork_parent_session_id: None,
            };
            assert_eq!(
                reader.capabilities(&input),
                SourceCapabilities::uncharacterized(format),
                "{agent}: {path}"
            );
        }
    }
}
