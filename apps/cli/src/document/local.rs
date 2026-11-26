//! Project only the exact indexed local source. Never follow attachments or links.

use std::{fs::OpenOptions, io::Read, path::Path};

use antiburn_local::analysis::{
    ClaudeSessionReader, ContentKind, NormalizedRecord, PartialReason, RawSource, RecordSink,
    Role as LocalRole, SessionInput, SessionReader, SessionSummary, SourceFormat, TurnContent,
    reader_for_input,
};

use crate::checks::SessionIdentity;

use super::{
    Agent, Coverage, CoverageLevel, Document, Header, Limitation, MAX_DOCUMENT_BYTES, Message,
    MessageDetails, Part, PartDetails, Payload, Role, SCHEMA_VERSION, Sections, SessionDocument,
    Source,
};

const MAX_SOURCE_BYTES: u64 = 16 * 1024 * 1024;

/// Source bindings come from the exact identity row in the read-only index.
pub(crate) struct LocalSource {
    pub identity: SessionIdentity,
    pub kind: String,
    pub label: String,
    pub format: Option<String>,
    pub title: Option<String>,
}

/// Read one bounded file snapshot and project it with the local engine's reader.
pub(crate) fn read_local(source: LocalSource) -> Result<SessionDocument, String> {
    let format = supported_format(&source)?;
    let path = Path::new(&source.label);
    let (content, tail_incomplete) = read_snapshot(path)?;
    let input = SessionInput {
        agent: source.identity.agent,
        session_id: source.identity.session_id,
        source: RawSource::Jsonl(content),
        source_format: format,
        fork_parent_session_id: None,
    };
    let mut sink = Projection {
        partial: tail_incomplete,
        ..Projection::default()
    };
    let reader: &dyn SessionReader = if format == SourceFormat::ClaudeJsonl {
        &ClaudeSessionReader
    } else {
        reader_for_input(&input)
    };
    reader
        .visit(&input, &mut sink)
        .map_err(|_| "the local session source could not be parsed")?;
    if sink.oversized {
        return Err("local session document exceeds the size limit".into());
    }
    if sink.messages.is_empty() {
        return Err("the local session source has no supported message content".into());
    }
    Ok(SessionDocument {
        schema_version: SCHEMA_VERSION,
        source: Source::Local,
        workspace_id: None,
        session_id: input.session_id,
        revision: None,
        stale: None,
        document: Document {
            version: SCHEMA_VERSION,
            header: Header {
                agent: Agent {
                    name: input.agent,
                    version: None,
                },
                title: source.title,
                execution_origin: None,
                collection_route: None,
                started_at: None,
                ended_at: None,
                outcome: None,
            },
            coverage: Coverage {
                source: if sink.partial {
                    CoverageLevel::Partial
                } else {
                    CoverageLevel::Unknown
                },
                projection: CoverageLevel::Partial,
                ordering: "presentation".into(),
                scope_description: Some(
                    "Indexed local session. Excludes child sessions and attachments.".into(),
                ),
                limitations: Some(
                    [
                        "header",
                        "messages",
                        "requests",
                        "context",
                        "usage",
                        "billing",
                        "artifacts",
                    ]
                    .into_iter()
                    .map(|area| Limitation {
                        area: area.into(),
                        reason: "local_projection_limited".into(),
                        note: None,
                        start: None,
                        end: None,
                    })
                    .collect(),
                ),
            },
            messages: sink.messages,
            sections: Sections::default(),
        },
    })
}

fn supported_format(source: &LocalSource) -> Result<SourceFormat, String> {
    let unsupported = "this local session source format is not supported by get";
    if source.identity.environment_key != "native" {
        return Err("get cannot read this local session environment".into());
    }
    if source.kind != "file" {
        return Err(unsupported.into());
    }
    let format = source
        .format
        .as_deref()
        .and_then(|value| serde_json::from_str(value).ok());
    match (source.identity.agent.as_str(), format) {
        ("claude-code", Some(format @ SourceFormat::ClaudeJsonl))
        | ("codex", Some(format @ SourceFormat::CodexRolloutJsonl))
        | ("pi", Some(format @ SourceFormat::PiV3Jsonl))
        | ("cursor", Some(format @ SourceFormat::CursorCliAgentJsonl)) => Ok(format),
        _ => Err(unsupported.into()),
    }
}

/// Copy at most one observed length. Reject changes instead of mixing file versions.
fn read_snapshot(path: &Path) -> Result<(String, bool), String> {
    let unavailable = "the indexed local session file is missing or unreadable";
    if !path.is_absolute() {
        return Err(unavailable.into());
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let mut file = options.open(path).map_err(|_| unavailable)?;
    let before = file.metadata().map_err(|_| unavailable)?;
    if !before.is_file() {
        return Err(unavailable.into());
    }
    if before.len() > MAX_SOURCE_BYTES {
        return Err("local session source exceeds the size limit".into());
    }
    let mut bytes = Vec::new();
    (&mut file)
        .take(before.len())
        .read_to_end(&mut bytes)
        .map_err(|_| unavailable)?;
    let after = file.metadata().map_err(|_| unavailable)?;
    if after.len() != bytes.len() as u64
        || before.len() != after.len()
        || before.modified().map_err(|_| unavailable)?
            != after.modified().map_err(|_| unavailable)?
    {
        return Err("the local session file changed during the read; try again".into());
    }
    let tail_incomplete = bytes.last().is_some_and(|byte| *byte != b'\n');
    if tail_incomplete {
        bytes.truncate(
            bytes
                .iter()
                .rposition(|byte| *byte == b'\n')
                .map_or(0, |index| index + 1),
        );
    }
    let content = String::from_utf8(bytes).map_err(|_| "the local session source is not UTF-8")?;
    Ok((content, tail_incomplete))
}

#[derive(Default)]
struct Projection {
    messages: Vec<Message>,
    role: Option<Role>,
    bytes: usize,
    partial: bool,
    oversized: bool,
}

impl Projection {
    fn message(&mut self, turn: TurnContent) {
        let Some(role) = self.role.take() else {
            self.partial = true;
            return;
        };
        let id = format!("message:{}", self.messages.len());
        let mut parts = Vec::new();
        for part in turn.parts {
            self.partial |= part.truncated;
            self.bytes += std::mem::size_of::<Part>()
                + part.text.len()
                + part.tool_name.as_ref().map_or(0, String::len);
            if self.bytes > MAX_DOCUMENT_BYTES {
                self.oversized = true;
                return;
            }
            let mut details = PartDetails::default();
            let (kind, text) = match part.kind {
                ContentKind::UserText | ContentKind::AssistantText => ("text", Some(part.text)),
                ContentKind::Thinking => {
                    details.representation = Some("source_text".into());
                    ("thinking", Some(part.text))
                }
                ContentKind::ToolInput => {
                    details.name = part.tool_name;
                    details.input = Some(Payload::JsonText {
                        value: part.text,
                        validity: "unknown".into(),
                    });
                    ("tool_call", None)
                }
                ContentKind::ToolResult => {
                    details.name = part.tool_name;
                    details.output = Some(vec![Payload::Text { value: part.text }]);
                    ("tool_result", None)
                }
            };
            parts.push(Part {
                id: format!("{id}:{}", parts.len()),
                kind: kind.into(),
                content_state: if part.truncated {
                    "partial"
                } else {
                    "complete"
                }
                .into(),
                text,
                details,
            });
        }
        if !parts.is_empty() {
            self.messages.push(Message {
                id,
                role,
                parts,
                recorded_at: None,
                details: MessageDetails::default(),
            });
        }
    }
}

impl RecordSink for Projection {
    fn tool_results_requested(&self) -> bool {
        false
    }

    fn record(&mut self, record: NormalizedRecord) {
        if self.oversized {
            return;
        }
        match record {
            NormalizedRecord::MetricsEvent(event) => {
                self.role = Some(match event.role {
                    LocalRole::User => Role::User,
                    LocalRole::Assistant => Role::Assistant,
                    LocalRole::Tool => Role::Tool,
                    LocalRole::System => Role::System,
                });
            }
            NormalizedRecord::TurnContent(turn) => self.message(*turn),
            NormalizedRecord::Unusable(reason) => {
                self.oversized = reason == PartialReason::Oversized;
                self.partial = true;
            }
            _ => {}
        }
    }

    fn finish(&mut self, summary: SessionSummary) {
        self.partial |= !summary.coverage_gaps.is_empty();
    }
}
