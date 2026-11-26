//! Versioned session document shared by local reads and the Cloud client.

use std::io::{self, Write};

#[cfg(feature = "cloud")]
use serde::Deserialize;
use serde::Serialize;
use serde_json::{Map, Value};

mod local;
pub(crate) use local::{IndexedDatabase, LocalSource, read_local};

const SCHEMA_VERSION: u8 = 1;
pub(crate) const MAX_DOCUMENT_BYTES: usize = 20 * 1024 * 1024;
// DEL is one byte in the response but takes six bytes in either output format.
// Leave extra room for labels and framing.
const MAX_DOCUMENT_OUTPUT_BYTES: usize = 6 * MAX_DOCUMENT_BYTES + 1024;
const DOCUMENT_LIMIT_ERROR: &str = "could not encode the session document within the size limit";
#[cfg(feature = "cloud")]
const INVALID_CLOUD_RESPONSE: &str = "Cloud returned an invalid response";

/// One public document. No raw records or source locators enter this type.
#[derive(Serialize)]
#[cfg_attr(feature = "cloud", derive(Deserialize))]
pub(crate) struct SessionDocument {
    schema_version: u8,
    source: Source,
    #[serde(skip_serializing_if = "Option::is_none")]
    workspace_id: Option<String>,
    session_id: String,
    #[serde(skip_deserializing, skip_serializing_if = "Option::is_none")]
    environment_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    revision: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stale: Option<bool>,
    document: Document,
}

#[derive(Serialize, PartialEq, Eq)]
#[cfg_attr(feature = "cloud", derive(Deserialize))]
#[serde(rename_all = "snake_case")]
enum Source {
    Local,
    #[cfg(feature = "cloud")]
    Cloud,
}

impl Source {
    fn label(&self) -> &'static str {
        match self {
            Self::Local => "local",
            #[cfg(feature = "cloud")]
            Self::Cloud => "cloud",
        }
    }
}

#[derive(Serialize)]
#[cfg_attr(feature = "cloud", derive(Deserialize))]
struct Document {
    version: u8,
    header: Header,
    messages: Vec<Message>,
    coverage: Coverage,
    #[serde(flatten)]
    sections: Sections,
}

/// Public sections already projected by the service. Other top-level keys are ignored.
#[derive(Default, Serialize)]
#[cfg_attr(feature = "cloud", derive(Deserialize))]
struct Sections {
    #[serde(skip_serializing_if = "Option::is_none")]
    requests: Option<Vec<Map<String, Value>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    response_cycles: Option<Vec<Map<String, Value>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    context: Option<Vec<Map<String, Value>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    usage: Option<Map<String, Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    billing: Option<Map<String, Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    native_only: Option<Vec<NativeOnly>>,
}

#[derive(Serialize)]
#[cfg_attr(feature = "cloud", derive(Deserialize))]
struct Header {
    agent: Agent,
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    execution_origin: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    collection_route: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    started_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    ended_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    outcome: Option<Outcome>,
}

#[derive(Serialize)]
#[cfg_attr(feature = "cloud", derive(Deserialize))]
struct Agent {
    name: String,
    version: Option<String>,
}

#[derive(Serialize)]
#[cfg_attr(feature = "cloud", derive(Deserialize))]
struct Coverage {
    source: CoverageLevel,
    projection: CoverageLevel,
    ordering: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    scope_description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    limitations: Option<Vec<Limitation>>,
}

#[derive(Serialize)]
#[cfg_attr(feature = "cloud", derive(Deserialize))]
struct Limitation {
    area: String,
    reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    note: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    start: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    end: Option<String>,
}

#[derive(Serialize)]
#[cfg_attr(feature = "cloud", derive(Deserialize))]
struct NativeOnly {
    id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    native_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
}

/// A complete projection does not prove the source retained the entire session.
#[derive(Serialize, PartialEq, Eq)]
#[cfg_attr(feature = "cloud", derive(Deserialize))]
#[serde(rename_all = "snake_case")]
enum CoverageLevel {
    #[cfg(feature = "cloud")]
    Complete,
    Partial,
    Unknown,
}

impl CoverageLevel {
    fn label(&self) -> &'static str {
        match self {
            #[cfg(feature = "cloud")]
            Self::Complete => "complete",
            Self::Partial => "partial",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Serialize)]
#[cfg_attr(feature = "cloud", derive(Deserialize))]
struct Message {
    id: String,
    role: Role,
    parts: Vec<Part>,
    #[serde(skip_serializing_if = "Option::is_none")]
    recorded_at: Option<String>,
    #[serde(flatten)]
    details: MessageDetails,
}

#[derive(Default, Serialize)]
#[cfg_attr(feature = "cloud", derive(Deserialize))]
struct MessageDetails {
    #[serde(skip_serializing_if = "Option::is_none")]
    origin: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    history_origin: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    parent_message_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    phase: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    outcome: Option<Outcome>,
}

#[derive(Serialize)]
#[cfg_attr(feature = "cloud", derive(Deserialize))]
struct Outcome {
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    terminal: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    native_status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    exit_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    signal: Option<String>,
}

#[derive(Serialize)]
#[cfg_attr(feature = "cloud", derive(Deserialize))]
struct Part {
    id: String,
    #[serde(rename = "type")]
    kind: String,
    content_state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<String>,
    #[serde(flatten)]
    details: PartDetails,
}

#[derive(Default, Serialize)]
#[cfg_attr(feature = "cloud", derive(Deserialize))]
struct PartDetails {
    #[serde(skip_serializing_if = "Option::is_none")]
    unavailable_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    request_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    format: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    representation: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    namespace: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    origin: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_definition_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    parent_call_part_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    retry_of_part_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    input: Option<Payload>,
    #[serde(skip_serializing_if = "Option::is_none")]
    outcome: Option<Outcome>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_part_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    output: Option<Vec<Payload>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    body_state: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    content_part_ids: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    skill_definition_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    semantic_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    value: Option<Payload>,
    #[serde(skip_serializing_if = "Option::is_none")]
    native_type: Option<String>,
}

#[derive(Serialize)]
#[cfg_attr(feature = "cloud", derive(Deserialize))]
#[serde(tag = "format", rename_all = "snake_case")]
enum Payload {
    Text {
        value: String,
    },
    #[cfg(feature = "cloud")]
    Json {
        value: Value,
    },
    JsonText {
        value: String,
        validity: String,
    },
}

#[derive(Serialize)]
#[cfg_attr(feature = "cloud", derive(Deserialize))]
#[serde(rename_all = "snake_case")]
enum Role {
    User,
    Assistant,
    System,
    #[cfg(feature = "cloud")]
    Developer,
    Tool,
    #[cfg_attr(feature = "cloud", serde(other))]
    Unknown,
}

impl SessionDocument {
    /// Parse supported documents once. If decoding fails, check the envelope version first.
    #[cfg(feature = "cloud")]
    pub(crate) fn decode_cloud(bytes: &[u8]) -> Result<Self, String> {
        let unsupported = "Cloud returned an unsupported document version";
        if let Ok(document) = serde_json::from_slice::<Self>(bytes) {
            return if document.schema_version == SCHEMA_VERSION
                && document.document.version == SCHEMA_VERSION
            {
                Ok(document)
            } else {
                Err(unsupported.into())
            };
        }

        #[derive(Deserialize)]
        struct EnvelopeVersion {
            schema_version: u64,
        }

        #[derive(Deserialize)]
        struct DocumentEnvelope {
            document: DocumentVersion,
        }

        #[derive(Deserialize)]
        struct DocumentVersion {
            version: u64,
        }

        let envelope: EnvelopeVersion =
            serde_json::from_slice(bytes).map_err(|_| INVALID_CLOUD_RESPONSE)?;
        let supported = u64::from(SCHEMA_VERSION);
        if envelope.schema_version != supported {
            return Err(unsupported.into());
        }
        let envelope: DocumentEnvelope =
            serde_json::from_slice(bytes).map_err(|_| INVALID_CLOUD_RESPONSE)?;
        if envelope.document.version != supported {
            return Err(unsupported.into());
        }
        Err(INVALID_CLOUD_RESPONSE.into())
    }

    #[cfg(feature = "cloud")]
    pub(crate) fn validate_cloud(
        &self,
        workspace_id: uuid::Uuid,
        session_id: uuid::Uuid,
    ) -> Result<(), String> {
        if self.source != Source::Cloud
            || self
                .workspace_id
                .as_deref()
                .and_then(|value| uuid::Uuid::parse_str(value).ok())
                != Some(workspace_id)
            || uuid::Uuid::parse_str(&self.session_id).ok() != Some(session_id)
            || self
                .revision
                .as_deref()
                .and_then(|value| uuid::Uuid::parse_str(value).ok())
                .is_none_or(|value| value.is_nil())
            || self.stale.is_none()
        {
            return Err("Cloud document does not match the request".into());
        }
        Ok(())
    }

    /// Serialize the typed document, escaping terminal control and format characters.
    pub(crate) fn to_json(&self) -> Result<Vec<u8>, String> {
        let mut output = DocumentBytes(Vec::new());
        let mut serializer = serde_json::Serializer::with_formatter(&mut output, SafeJson);
        self.serialize(&mut serializer)
            .map_err(|_| DOCUMENT_LIMIT_ERROR)?;
        output.write_all(b"\n").map_err(|_| DOCUMENT_LIMIT_ERROR)?;
        Ok(output.0)
    }

    /// Escape captured text so it cannot control the terminal.
    pub(crate) fn to_text(&self) -> Result<Vec<u8>, String> {
        let mut output = DocumentBytes(Vec::new());
        self.write_text(&mut output)
            .map_err(|_| DOCUMENT_LIMIT_ERROR)?;
        Ok(output.0)
    }

    fn write_text(&self, output: &mut impl Write) -> io::Result<()> {
        let header = &self.document.header;
        write_escaped_text(
            output,
            header.title.as_deref().unwrap_or("Untitled session"),
        )?;
        output.write_all(b"\nSession ")?;
        write_escaped_text(output, &self.session_id)?;
        writeln!(output, " ({})", self.source.label())?;
        output.write_all(b"Agent: ")?;
        write_escaped_text(output, &header.agent.name)?;
        output.write_all(b"\n")?;
        if let Some(environment) = &self.environment_key {
            output.write_all(b"Environment: ")?;
            write_escaped_text(output, environment)?;
            output.write_all(b"\n")?;
        }
        if self.stale == Some(true) {
            output.write_all(b"Cloud returned an older publication.\n")?;
        }
        if let Some(revision) = &self.revision {
            writeln!(output, "Revision: {revision}")?;
        }
        writeln!(
            output,
            "Source coverage: {}",
            self.document.coverage.source.label()
        )?;
        if self.document.coverage.projection == CoverageLevel::Partial {
            output.write_all(b"Document projection is partial.\n")?;
        }
        writeln!(output, "Messages: {}", self.document.messages.len())?;
        for message in &self.document.messages {
            let role = match message.role {
                Role::User => "User",
                Role::Assistant => "Assistant",
                Role::System => "System",
                #[cfg(feature = "cloud")]
                Role::Developer => "Developer",
                Role::Tool => "Tool",
                Role::Unknown => "Unknown",
            };
            for part in &message.parts {
                if matches!(part.kind.as_str(), "text" | "refusal")
                    && let Some(text) = &part.text
                {
                    write!(output, "{role}: \"")?;
                    write_escaped_text(output, text)?;
                    output.write_all(b"\"\n")?;
                }
            }
        }
        Ok(())
    }
}

/// Keep debug escaping for terminal safety, but leave apostrophes literal.
fn write_escaped_text(output: &mut impl Write, text: &str) -> io::Result<()> {
    for (index, segment) in text.split('\'').enumerate() {
        if index != 0 {
            output.write_all(b"'")?;
        }
        write!(output, "{}", segment.escape_debug())?;
    }
    Ok(())
}

struct DocumentBytes(Vec<u8>);

impl Write for DocumentBytes {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > MAX_DOCUMENT_OUTPUT_BYTES - self.0.len() {
            return Err(io::Error::other("session document exceeds the size limit"));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

struct SafeJson;

impl serde_json::ser::Formatter for SafeJson {
    fn write_string_fragment<W: Write + ?Sized>(
        &mut self,
        writer: &mut W,
        fragment: &str,
    ) -> io::Result<()> {
        let mut start = 0;
        for (index, character) in fragment.char_indices() {
            if character.is_control()
                || matches!(
                    character,
                    '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{2028}'..='\u{202e}' | '\u{2066}'..='\u{2069}'
                )
            {
                writer.write_all(&fragment.as_bytes()[start..index])?;
                write!(writer, "\\u{:04x}", character as u32)?;
                start = index + character.len_utf8();
            }
        }
        writer.write_all(&fragment.as_bytes()[start..])
    }
}
