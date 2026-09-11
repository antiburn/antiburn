//! Dedicated fail-closed readers for formats without detector-grade contracts.

use super::generic_jsonl::GenericJsonlSessionReader;
use crate::analysis::interface::{
    RawSource, RecordSink, SessionInput, SessionReader, VisitOutcome,
};
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

    fn capabilities(&self, source: &RawSource) -> SourceCapabilities {
        SourceCapabilities::uncharacterized(format_for(self.agent, self.format, source))
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

fn format_for(agent: &str, default: SourceFormat, source: &RawSource) -> SourceFormat {
    let RawSource::File(path) = source else {
        return default;
    };
    let path = path.to_string_lossy().to_ascii_lowercase();
    match agent {
        "copilot" if path.ends_with("events.jsonl") => SourceFormat::CopilotCliJsonl,
        "copilot" => SourceFormat::CopilotIdeChatJson,
        "kiro" if path.ends_with(".chat") => SourceFormat::KiroChat,
        "amp-code" if path.contains("file-changes") => SourceFormat::AmpFileChanges,
        "windsurf" if path.ends_with(".pb") => SourceFormat::WindsurfCascadeProtobuf,
        "windsurf" if path.contains("mirror") => SourceFormat::WindsurfMirrorJson,
        _ => default,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_variants_keep_fail_closed_capabilities() {
        for (agent, path, format) in [
            ("copilot", "events.jsonl", SourceFormat::CopilotCliJsonl),
            ("copilot", "chat.json", SourceFormat::CopilotIdeChatJson),
            ("cline", "session.json", SourceFormat::ClineSessionJson),
            ("kiro", "session.json", SourceFormat::KiroSessionJson),
            ("kiro", "session.chat", SourceFormat::KiroChat),
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
            assert_eq!(
                reader.capabilities(&RawSource::File(path.into())),
                SourceCapabilities::uncharacterized(format),
                "{agent}: {path}"
            );
        }
    }
}
