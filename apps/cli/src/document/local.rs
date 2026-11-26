//! Project one indexed local session. Never follow attachments or links.

use std::{
    fs::OpenOptions,
    io::Read,
    path::Path,
    time::{Duration, Instant},
};

use antiburn_local::analysis::{
    ContentAuthority, ContentKind, NormalizedRecord, OpenCodeV2SnapshotError, PartialReason,
    RawSource, RecordSink, Role as LocalRole, SessionInput, SessionSummary, SourceFormat,
    TurnContent, VisitOutcome, reader_for_input,
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
    pub database: Option<IndexedDatabase>,
}

/// Desktop's stored path and fingerprint of an admitted provider database session.
pub(crate) struct IndexedDatabase {
    pub path: String,
    pub fingerprint: String,
}

impl LocalSource {
    pub(crate) fn is_opencode_v2(&self) -> bool {
        self.identity.environment_key == "native"
            && self.identity.agent == "opencode"
            && self.kind == "providerDb"
            && self.source_format() == Some(SourceFormat::OpenCodeSqliteV2)
    }

    fn source_format(&self) -> Option<SourceFormat> {
        self.format
            .as_deref()
            .and_then(|value| serde_json::from_str(value).ok())
    }
}

/// Read a bounded snapshot and project it with the local engine's reader.
pub(crate) fn read_local(source: LocalSource) -> Result<SessionDocument, String> {
    let (format, raw, tail_incomplete, fingerprint) = if source.is_opencode_v2() {
        let binding = source.database.ok_or("the indexed OpenCode V2 database is not ready; refresh the session in Antiburn Desktop")?;
        let path = Path::new(&binding.path);
        if !path.is_absolute()
            || !path
                .symlink_metadata()
                .is_ok_and(|metadata| metadata.is_file())
        {
            return Err("the indexed OpenCode V2 database is missing or unreadable".into());
        }
        (
            SourceFormat::OpenCodeSqliteV2,
            RawSource::Sqlite(path.to_path_buf()),
            false,
            Some(binding.fingerprint),
        )
    } else {
        let format = supported_file_format(&source)?;
        let (content, tail_incomplete) = read_snapshot(Path::new(&source.label))?;
        (format, RawSource::Jsonl(content), tail_incomplete, None)
    };
    let input = SessionInput {
        agent: source.identity.agent,
        session_id: source.identity.session_id,
        source: raw,
        source_format: format,
        fork_parent_session_id: None,
    };
    let mut sink = Projection {
        partial: tail_incomplete,
        database_session_id: fingerprint.as_ref().map(|_| input.session_id.clone()),
        ..Projection::default()
    };
    visit_source(&input, fingerprint.as_deref(), &mut sink)?;
    if sink.capacity_exceeded {
        return Err("local session document exceeds the size limit".into());
    }
    if sink.messages.is_empty() {
        if tail_incomplete {
            return Err("the local session file has an incomplete record; try again".into());
        }
        if !sink.record_oversized {
            return Err("the local session source has no supported message content".into());
        }
    }
    Ok(local_document(
        input,
        source.identity.environment_key,
        source.title,
        sink,
    ))
}

fn visit_source(
    input: &SessionInput,
    fingerprint: Option<&str>,
    sink: &mut Projection,
) -> Result<(), String> {
    if let Some(fingerprint) = fingerprint {
        let deadline = Instant::now() + Duration::from_secs(5);
        let cancel = || Instant::now() >= deadline;
        return match reader_for_input(input).visit_db_claimed(input, fingerprint, &cancel, sink) {
            Ok(VisitOutcome::AcceptedFull) => Ok(()),
            Ok(_) => Err(
                "the indexed OpenCode V2 source changed; refresh the session in Antiburn Desktop"
                    .into(),
            ),
            Err(_) if cancel() => Err("the local OpenCode V2 read exceeded the time limit".into()),
            Err(error)
                if error.downcast_ref::<OpenCodeV2SnapshotError>()
                    == Some(&OpenCodeV2SnapshotError::SourceLimit) =>
            {
                Err("the local OpenCode V2 source exceeds the size limit".into())
            }
            Err(_) => Err(
                "the indexed OpenCode V2 database could not be read or has an unsupported schema"
                    .into(),
            ),
        };
    }
    reader_for_input(input)
        .visit(input, sink)
        .map_err(|_| "the local session source could not be parsed")?;
    Ok(())
}

fn local_document(
    input: SessionInput,
    environment_key: String,
    title: Option<String>,
    sink: Projection,
) -> SessionDocument {
    SessionDocument {
        schema_version: SCHEMA_VERSION,
        source: Source::Local,
        workspace_id: None,
        session_id: input.session_id,
        environment_key: Some(environment_key),
        revision: None,
        stale: None,
        document: Document {
            version: SCHEMA_VERSION,
            header: Header {
                agent: Agent {
                    name: input.agent,
                    version: None,
                },
                title,
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
    }
}

fn supported_file_format(source: &LocalSource) -> Result<SourceFormat, String> {
    let unsupported = "this local session source format is not supported by get";
    if source.identity.environment_key != "native" {
        return Err("get cannot read this local session environment".into());
    }
    if source.kind != "file" {
        return Err(unsupported.into());
    }
    match (source.identity.agent.as_str(), source.source_format()) {
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
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;

        // Open the final component itself so the handle can identify a reparse point.
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let mut file = options.open(path).map_err(|_| unavailable)?;
    let before = file.metadata().map_err(|_| unavailable)?;
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;

        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
        if before.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(unavailable.into());
        }
    }
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
    let mut tail_incomplete = false;
    if bytes.last().is_some_and(|byte| *byte != b'\n') {
        let tail_start = bytes
            .iter()
            .rposition(|byte| *byte == b'\n')
            .map_or(0, |index| index + 1);
        let tail = &bytes[tail_start..];
        let is_object = tail.iter().find(|byte| !byte.is_ascii_whitespace()) == Some(&b'{');
        if is_object
            && std::str::from_utf8(tail)
                .is_ok_and(|tail| serde_json::from_str::<serde::de::IgnoredAny>(tail).is_ok())
        {
            // Some readers require a line separator even at the end of the snapshot.
            bytes.push(b'\n');
        } else {
            tail_incomplete = true;
            bytes.truncate(tail_start);
        }
    }
    let content = String::from_utf8(bytes).map_err(|_| "the local session source is not UTF-8")?;
    Ok((content, tail_incomplete))
}

#[derive(Default)]
struct Projection {
    messages: Vec<Message>,
    role: Option<LocalRole>,
    bytes: usize,
    partial: bool,
    record_oversized: bool,
    capacity_exceeded: bool,
    database_session_id: Option<String>,
    ignore_source: bool,
}

impl Projection {
    fn message(&mut self, turn: TurnContent) {
        let Some(role) = self.role.take() else {
            self.partial = true;
            return;
        };
        let mut parts = Vec::new();
        let mut unknown = false;
        for part in turn.parts {
            self.partial |= part.truncated;
            self.bytes += std::mem::size_of::<Part>()
                + part.text.len()
                + part.tool_name.as_ref().map_or(0, String::len);
            if self.bytes > MAX_DOCUMENT_BYTES {
                self.capacity_exceeded = true;
                return;
            }
            // A user event can contain text that the source does not attribute to the user.
            let part_unknown = role == LocalRole::User
                && part.kind == ContentKind::UserText
                && part.authority != ContentAuthority::User;
            if part_unknown != unknown && !parts.is_empty() {
                self.push_message(role, unknown, std::mem::take(&mut parts));
            }
            unknown = part_unknown;
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
                id: format!("message:{}:{}", self.messages.len(), parts.len()),
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
            self.push_message(role, unknown, parts);
        }
    }

    fn push_message(&mut self, role: LocalRole, unknown: bool, parts: Vec<Part>) {
        let role = match role {
            LocalRole::User if unknown => Role::Unknown,
            LocalRole::User => Role::User,
            LocalRole::Assistant => Role::Assistant,
            LocalRole::Tool => Role::Tool,
            LocalRole::System => Role::System,
        };
        self.messages.push(Message {
            id: format!("message:{}", self.messages.len()),
            role,
            parts,
            recorded_at: None,
            details: MessageDetails::default(),
        });
    }
}

impl RecordSink for Projection {
    fn begin_source(&mut self, session_id: &str) {
        self.ignore_source = self
            .database_session_id
            .as_deref()
            .is_some_and(|id| id != session_id);
        self.role = None;
    }

    fn source_summary(&mut self, session_id: &str, summary: SessionSummary) {
        if self.database_session_id.as_deref() == Some(session_id) {
            self.partial |= !summary.coverage_gaps.is_empty();
        }
    }

    fn tool_results_requested(&self) -> bool {
        false
    }

    fn record(&mut self, record: NormalizedRecord) {
        if self.capacity_exceeded || self.ignore_source {
            return;
        }
        match record {
            NormalizedRecord::MetricsEvent(event) => self.role = Some(event.role),
            NormalizedRecord::TurnContent(turn) => self.message(*turn),
            NormalizedRecord::Unusable(reason) => {
                self.record_oversized |= reason == PartialReason::Oversized;
                self.capacity_exceeded =
                    self.database_session_id.is_some() && reason == PartialReason::CapacityExceeded;
                self.partial = true;
            }
            _ => {}
        }
    }

    fn finish(&mut self, summary: SessionSummary) {
        if self.database_session_id.is_none() {
            self.partial |= !summary.coverage_gaps.is_empty();
        }
    }
}

#[cfg(test)]
#[path = "tests/local_tests.rs"]
mod tests;
