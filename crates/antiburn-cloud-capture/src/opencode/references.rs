//! Bounded extraction at defined OpenCode reference fields.
//!
//! This reader never constructs a native JSON tree or retains message text. It
//! decodes strings through a fixed prefix buffer, including ignored large values.
//! Only typed file fields and instruction maps can select content-addressed rows.
//! Unknown native record kinds and unknown columns are not reference authorities.

use super::{MAX_REFERENCES, check_deadline, sql_error};
use crate::{Error, Result};
use rusqlite::{Connection, DatabaseName};
use sha2::{Digest, Sha256};
use std::cell::Cell;
use std::collections::BTreeSet;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::time::Instant;

/// A pinned native field context, not permission to search arbitrary JSON.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Shape {
    Opaque,
    User,
    Assistant,
    ToolPart,
    Part,
    Kind,
    ToolState,
    ToolEvent,
    InstructionsEvent,
    ForkEvent,
    InboxEvent,
    InboxItem,
    UserItem,
    InstructionValues,
    InstructionDelta,
    Files,
    Parts,
    Contents,
    File,
    Content,
}

/// Distinct selected dependencies and a shared native-read budget for one export.
pub(super) struct References {
    pub media: BTreeSet<String>,
    pub instructions: BTreeSet<String>,
    maximum_bytes: u64,
    bytes_read: Cell<u64>,
    deadline: Instant,
}

impl References {
    /// Keep one deadline and byte budget across every dependency-bearing cell.
    pub(super) fn new(maximum_bytes: u64, deadline: Instant) -> Self {
        Self {
            media: BTreeSet::new(),
            instructions: BTreeSet::new(),
            maximum_bytes,
            bytes_read: Cell::new(0),
            deadline,
        }
    }

    /// Accept a hash only after the caller proves its native session context.
    pub(super) fn add_media(&mut self, hash: &str) -> Result<()> {
        validate_hash(hash)?;
        if !self.media.contains(hash)
            && self.media.len() + self.instructions.len() == MAX_REFERENCES
        {
            return Err(Error::Limit);
        }
        self.media.insert(hash.into());
        Ok(())
    }

    fn add_instruction(&mut self, hash: &str) -> Result<()> {
        validate_hash(hash)?;
        if !self.instructions.contains(hash)
            && self.media.len() + self.instructions.len() == MAX_REFERENCES
        {
            return Err(Error::Limit);
        }
        self.instructions.insert(hash.into());
        Ok(())
    }
}

fn validate_hash(hash: &str) -> Result<()> {
    if hash.len() != 64
        || !hash
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(Error::InvalidSource);
    }
    Ok(())
}

/// Read a defined dependency-bearing cell without changing its archived bytes.
///
/// The caller selects owned rows first. Known event roots must carry the supplied
/// session ID. Non-UTF-8 stores and ambiguous dependency fields fail rather than
/// selecting unproved shared content. The caller retains opaque kinds separately.
pub(super) fn scan(
    connection: &Connection,
    table: &str,
    column: &str,
    rowid: i64,
    shape: Shape,
    references: &mut References,
    session_id: Option<&str>,
) -> Result<()> {
    check_deadline(references.deadline)?;
    let encoding: String = connection
        .query_row("PRAGMA encoding", [], |row| row.get(0))
        .map_err(sql_error)?;
    // OpenCode's characterized stores use UTF-8. Do not reinterpret a converted
    // database's dependency fields while claiming the same selection contract.
    if encoding != "UTF-8" {
        return Err(Error::UnsupportedSource);
    }
    let kind: String = connection
        .query_row(
            &format!("SELECT typeof(\"{column}\") FROM \"{table}\" WHERE rowid = ?1"),
            [rowid],
            |row| row.get(0),
        )
        .map_err(sql_error)?;
    if kind != "text" {
        return Err(Error::InvalidSource);
    }
    let blob = connection
        .blob_open(DatabaseName::Main, table, column, rowid, true)
        .map_err(sql_error)?;
    if blob.len() as u64 > references.maximum_bytes {
        return Err(Error::Limit);
    }
    let total = references
        .bytes_read
        .get()
        .checked_add(blob.len() as u64)
        .ok_or(Error::Limit)?;
    if total > references.maximum_bytes {
        return Err(Error::Limit);
    }
    references.bytes_read.set(total);
    let mut reader = BufReader::with_capacity(8192, blob);
    Parser {
        reader: &mut reader,
        references,
        buffered: 0,
        kind: None,
        session_id,
    }
    .document(shape)
}

struct Parser<'a, R> {
    reader: &'a mut R,
    references: &'a mut References,
    buffered: usize,
    kind: Option<String>,
    session_id: Option<&'a str>,
}

/// Only this fixed prefix is retained. The digest detects repeated instruction
/// keys without retaining unbounded key strings or changing their native bytes.
struct Text {
    prefix: [u8; 256],
    len: usize,
    digest: [u8; 32],
}

impl Text {
    fn value(&self) -> Option<&str> {
        (self.len <= self.prefix.len())
            .then(|| std::str::from_utf8(&self.prefix[..self.len]).ok())
            .flatten()
    }
}

impl<R: BufRead + Seek> Parser<'_, R> {
    fn document(&mut self, shape: Shape) -> Result<()> {
        self.value(shape, 0)?;
        self.space()?;
        if self.peek()?.is_some() {
            return Err(Error::InvalidSource);
        }
        Ok(())
    }

    fn value(&mut self, shape: Shape, depth: usize) -> Result<bool> {
        if depth == 128 {
            return Err(Error::Limit);
        }
        self.space()?;
        match shape {
            Shape::Part => return self.typed_object("tool", Shape::ToolPart, depth),
            Shape::Content => return self.typed_object("file", Shape::File, depth),
            Shape::InboxItem => return self.typed_object("user", Shape::UserItem, depth),
            _ => {}
        }
        match self.peek()? {
            Some(b'{') => self.object(shape, depth + 1),
            Some(b'[') => self.array(shape, depth + 1),
            Some(b'"') => {
                let _ = self.text(false)?;
                if shape != Shape::Opaque {
                    return Err(Error::InvalidSource);
                }
                Ok(false)
            }
            Some(b'n') => {
                self.literal(b"null")?;
                if shape != Shape::Opaque {
                    return Err(Error::InvalidSource);
                }
                Ok(false)
            }
            Some(b't') => {
                self.literal(b"true")?;
                if shape != Shape::Opaque {
                    return Err(Error::InvalidSource);
                }
                Ok(false)
            }
            Some(b'f') => {
                self.literal(b"false")?;
                if shape != Shape::Opaque {
                    return Err(Error::InvalidSource);
                }
                Ok(false)
            }
            Some(b'-' | b'0'..=b'9') => {
                self.number()?;
                if shape != Shape::Opaque {
                    return Err(Error::InvalidSource);
                }
                Ok(false)
            }
            _ => Err(Error::InvalidSource),
        }
    }

    fn typed_object(&mut self, kind: &str, shape: Shape, depth: usize) -> Result<bool> {
        let start = self.reader.stream_position().map_err(Error::from)?;
        self.value(Shape::Kind, depth)?;
        if self.kind.take().as_deref() != Some(kind) {
            return Ok(false);
        }
        self.reader
            .seek(SeekFrom::Start(start))
            .map_err(Error::from)?;
        self.buffered = 0;
        self.value(shape, depth)
    }

    fn object(&mut self, shape: Shape, depth: usize) -> Result<bool> {
        if matches!(shape, Shape::Files | Shape::Parts | Shape::Contents) {
            return Err(Error::InvalidSource);
        }
        self.expect(b'{')?;
        let mut names = BTreeSet::new();
        let mut fields = BTreeSet::new();
        let mut kind = None;
        let mut status = None;
        let mut media = None;
        let mut mime = false;
        let mut found = false;
        let mut first = true;
        loop {
            self.space()?;
            if self.peek()? == Some(b'}') {
                self.expect(b'}')?;
                break;
            }
            if !first {
                self.expect(b',')?;
                self.space()?;
            }
            first = false;
            let key = self.text(true)?;
            self.space()?;
            self.expect(b':')?;
            self.space()?;
            if matches!(shape, Shape::InstructionValues | Shape::InstructionDelta) {
                if names.len() == MAX_REFERENCES {
                    return Err(Error::Limit);
                }
                if !names.insert(key.digest) {
                    return Err(Error::InvalidSource);
                }
                let hash = self.text(false)?;
                let hash = hash.value().ok_or(Error::InvalidSource)?;
                if shape == Shape::InstructionDelta && hash == "removed" {
                    continue;
                }
                self.references.add_instruction(hash)?;
                found = true;
                continue;
            }
            let key = key.value().unwrap_or("");
            let child = child_shape(shape, key);
            let scope_field = depth == 1 && self.session_id.is_some() && key == "sessionID";
            let scalar = scalar_field(shape, key) || scope_field;
            if (child != Shape::Opaque || scalar) && !fields.insert(key.to_owned()) {
                return Err(Error::InvalidSource);
            }
            if scalar {
                if shape == Shape::Kind && self.peek()? != Some(b'"') {
                    self.value(Shape::Opaque, depth)?;
                    continue;
                }
                let text = self.text(false)?;
                let value = text.value();
                match key {
                    "sessionID" if scope_field => {
                        if value != self.session_id {
                            return Err(Error::InvalidSource);
                        }
                    }
                    "type" => kind = value.map(str::to_owned),
                    "status" => status = value.map(str::to_owned),
                    "mime" => mime = text.len > 0,
                    "uri" if text.prefix.starts_with(b"media:") => {
                        let hash = value
                            .and_then(|value| value.strip_prefix("media:"))
                            .ok_or(Error::InvalidSource)?;
                        validate_hash(hash)?;
                        media = Some(hash.to_owned());
                    }
                    _ => {}
                }
            } else {
                found |= self.value(child, depth)?;
            }
        }
        if let Some(hash) = media {
            if !mime {
                return Err(Error::InvalidSource);
            }
            self.references.add_media(&hash)?;
            found = true;
        }
        if found
            && ((shape == Shape::ToolPart && kind.as_deref() != Some("tool"))
                || (shape == Shape::ToolState
                    && !matches!(status.as_deref(), Some("completed" | "error"))))
        {
            return Err(Error::InvalidSource);
        }
        if shape == Shape::InstructionsEvent && !fields.contains("delta") {
            return Err(Error::InvalidSource);
        }
        if depth == 1 && self.session_id.is_some() && !fields.contains("sessionID") {
            return Err(Error::InvalidSource);
        }
        if shape == Shape::Kind {
            self.kind = kind;
        }
        Ok(found)
    }

    fn array(&mut self, shape: Shape, depth: usize) -> Result<bool> {
        let child = match shape {
            Shape::Opaque => Shape::Opaque,
            Shape::Files => Shape::File,
            Shape::Parts => Shape::Part,
            Shape::Contents => Shape::Content,
            _ => return Err(Error::InvalidSource),
        };
        self.expect(b'[')?;
        let mut found = false;
        let mut first = true;
        loop {
            self.space()?;
            if self.peek()? == Some(b']') {
                self.expect(b']')?;
                return Ok(found);
            }
            if !first {
                self.expect(b',')?;
            }
            first = false;
            found |= self.value(child, depth)?;
        }
    }

    fn text(&mut self, digest: bool) -> Result<Text> {
        self.expect(b'"')?;
        let mut text = Text {
            prefix: [0; 256],
            len: 0,
            digest: [0; 32],
        };
        let mut hash = digest.then(Sha256::new);
        loop {
            let byte = self.take()?.ok_or(Error::InvalidSource)?;
            let mut encoded = [0u8; 4];
            let bytes: &[u8] = match byte {
                b'"' => break,
                0..=31 => return Err(Error::InvalidSource),
                b'\\' => {
                    let escaped = self.take()?.ok_or(Error::InvalidSource)?;
                    encoded[0] = match escaped {
                        b'"' | b'\\' | b'/' => escaped,
                        b'b' => 8,
                        b'f' => 12,
                        b'n' => b'\n',
                        b'r' => b'\r',
                        b't' => b'\t',
                        b'u' => {
                            let length = self.unicode_escape(&mut encoded)?;
                            append_text(&mut text, &encoded[..length], &mut hash)?;
                            continue;
                        }
                        _ => return Err(Error::InvalidSource),
                    };
                    &encoded[..1]
                }
                32..=127 => {
                    encoded[0] = byte;
                    &encoded[..1]
                }
                _ => {
                    let length = match byte {
                        0xc2..=0xdf => 2,
                        0xe0..=0xef => 3,
                        0xf0..=0xf4 => 4,
                        _ => return Err(Error::InvalidSource),
                    };
                    encoded[0] = byte;
                    for slot in &mut encoded[1..length] {
                        *slot = self.take()?.ok_or(Error::InvalidSource)?;
                    }
                    std::str::from_utf8(&encoded[..length]).map_err(|_| Error::InvalidSource)?;
                    &encoded[..length]
                }
            };
            append_text(&mut text, bytes, &mut hash)?;
        }
        if let Some(hash) = hash {
            text.digest = hash.finalize().into();
        }
        Ok(text)
    }

    /// Accept JSON.stringify's lone UTF-16 surrogates in opaque source strings.
    /// Their distinct non-UTF-8 prefix cannot become an ASCII field or reference.
    fn unicode_escape(&mut self, encoded: &mut [u8; 4]) -> Result<usize> {
        let high = self.hex_quad()?;
        let mut scalar = u32::from(high);
        if (0xd800..=0xdbff).contains(&high) && self.peek()? == Some(b'\\') {
            let next_escape = self.reader.stream_position().map_err(Error::from)?;
            self.take()?;
            if self.take()? == Some(b'u') {
                let low = self.hex_quad()?;
                if (0xdc00..=0xdfff).contains(&low) {
                    scalar = 0x10000 + ((u32::from(high) - 0xd800) << 10) + u32::from(low) - 0xdc00;
                }
            }
            if scalar == u32::from(high) {
                self.reader
                    .seek(SeekFrom::Start(next_escape))
                    .map_err(Error::from)?;
                self.buffered = 0;
            }
        }
        if let Some(character) = char::from_u32(scalar) {
            return Ok(character.encode_utf8(encoded).len());
        }
        encoded[0] = 0xe0 | ((scalar >> 12) as u8);
        encoded[1] = 0x80 | (((scalar >> 6) & 0x3f) as u8);
        encoded[2] = 0x80 | ((scalar & 0x3f) as u8);
        Ok(3)
    }

    fn hex_quad(&mut self) -> Result<u16> {
        let mut value = 0u16;
        for _ in 0..4 {
            let digit = match self.take()? {
                Some(byte @ b'0'..=b'9') => byte - b'0',
                Some(byte @ b'a'..=b'f') => byte - b'a' + 10,
                Some(byte @ b'A'..=b'F') => byte - b'A' + 10,
                _ => return Err(Error::InvalidSource),
            };
            value = value * 16 + u16::from(digit);
        }
        Ok(value)
    }

    fn number(&mut self) -> Result<()> {
        if self.peek()? == Some(b'-') {
            self.take()?;
        }
        match self.take()? {
            Some(b'0') => {}
            Some(b'1'..=b'9') => self.digits()?,
            _ => return Err(Error::InvalidSource),
        }
        if self.peek()? == Some(b'.') {
            self.take()?;
            self.required_digits()?;
        }
        if matches!(self.peek()?, Some(b'e' | b'E')) {
            self.take()?;
            if matches!(self.peek()?, Some(b'+' | b'-')) {
                self.take()?;
            }
            self.required_digits()?;
        }
        Ok(())
    }

    fn required_digits(&mut self) -> Result<()> {
        if !matches!(self.take()?, Some(b'0'..=b'9')) {
            return Err(Error::InvalidSource);
        }
        self.digits()
    }

    fn digits(&mut self) -> Result<()> {
        while matches!(self.peek()?, Some(b'0'..=b'9')) {
            self.take()?;
        }
        Ok(())
    }

    fn literal(&mut self, value: &[u8]) -> Result<()> {
        for byte in value {
            self.expect(*byte)?;
        }
        Ok(())
    }

    fn space(&mut self) -> Result<()> {
        while matches!(self.peek()?, Some(b' ' | b'\t' | b'\r' | b'\n')) {
            self.take()?;
        }
        Ok(())
    }

    fn expect(&mut self, expected: u8) -> Result<()> {
        if self.take()? != Some(expected) {
            return Err(Error::InvalidSource);
        }
        Ok(())
    }

    fn peek(&mut self) -> Result<Option<u8>> {
        let bytes = self.reader.fill_buf().map_err(Error::from)?;
        if self.buffered == 0 && !bytes.is_empty() {
            check_deadline(self.references.deadline)?;
            self.buffered = bytes.len();
        }
        Ok(bytes.first().copied())
    }

    fn take(&mut self) -> Result<Option<u8>> {
        let byte = self.peek()?;
        if byte.is_some() {
            self.reader.consume(1);
            self.buffered -= 1;
        }
        Ok(byte)
    }
}

fn append_text(text: &mut Text, bytes: &[u8], hash: &mut Option<Sha256>) -> Result<()> {
    let count = bytes.len().min(text.prefix.len().saturating_sub(text.len));
    if count > 0 {
        text.prefix[text.len..text.len + count].copy_from_slice(&bytes[..count]);
    }
    text.len = text.len.checked_add(bytes.len()).ok_or(Error::Limit)?;
    if let Some(hash) = hash {
        hash.update(bytes);
    }
    Ok(())
}

fn child_shape(shape: Shape, key: &str) -> Shape {
    match (shape, key) {
        (Shape::User, "files") => Shape::Files,
        (Shape::Assistant, "content") => Shape::Parts,
        (Shape::ToolPart, "state") => Shape::ToolState,
        (Shape::ToolState | Shape::ToolEvent, "content") => Shape::Contents,
        (Shape::InstructionsEvent, "delta") => Shape::InstructionDelta,
        (Shape::ForkEvent, "instructions") => Shape::InstructionValues,
        (Shape::InboxEvent, "item") => Shape::InboxItem,
        (Shape::UserItem, "payload") => Shape::User,
        _ => Shape::Opaque,
    }
}

fn scalar_field(shape: Shape, key: &str) -> bool {
    matches!(
        (shape, key),
        (Shape::Kind | Shape::ToolPart, "type")
            | (Shape::ToolState, "status")
            | (Shape::File, "uri" | "mime")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use std::time::Duration;

    fn parse(bytes: &[u8], shape: Shape) -> Result<References> {
        let mut refs =
            References::new(bytes.len() as u64, Instant::now() + Duration::from_secs(30));
        let mut input = Cursor::new(bytes);
        Parser {
            reader: &mut input,
            references: &mut refs,
            buffered: 0,
            kind: None,
            session_id: None,
        }
        .document(shape)?;
        Ok(refs)
    }

    #[test]
    fn ignores_large_text_and_hashes_outside_typed_fields() {
        let hash = "a".repeat(64);
        let json = format!(
            r#"{{"text":"{}","uri":"media:{hash}","metadata":{{"files":[{{"mime":"text/plain","uri":"media:{hash}"}}]}}}}"#,
            "x".repeat(2_000_000)
        );
        assert!(
            parse(json.as_bytes(), Shape::User)
                .unwrap()
                .media
                .is_empty()
        );
    }

    #[test]
    fn follows_escaped_typed_file_refs_and_instruction_values_only() {
        let hash = "a".repeat(64);
        let json = format!(r#"{{"files":[{{"mime":"text/plain","uri":"\u006dedia:{hash}"}}]}}"#);
        assert_eq!(
            parse(json.as_bytes(), Shape::User).unwrap().media,
            BTreeSet::from([hash.clone()])
        );
        let json =
            format!(r#"{{"delta":{{"api/test":"{hash}","api/old":"removed"}},"text":"ignored"}}"#);
        assert_eq!(
            parse(json.as_bytes(), Shape::InstructionsEvent)
                .unwrap()
                .instructions,
            BTreeSet::from([hash])
        );
    }

    #[test]
    fn rejects_duplicate_or_malformed_dependency_fields() {
        for input in [
            r#"{"delta":{},"delta":{}}"#,
            r#"{"delta":{"x":"removed","x":"removed"}}"#,
            r#"{"delta":{"x":null}}"#,
            r#"{"delta":{"x":"not-a-hash"}}"#,
            r#"{"delta":{},"x":[1,]}"#,
            r#"{"delta":{"x":"\uD800"}}"#,
        ] {
            assert!(matches!(
                parse(input.as_bytes(), Shape::InstructionsEvent),
                Err(Error::InvalidSource)
            ));
        }
    }

    #[test]
    fn keeps_lone_surrogates_and_provider_state_opaque() {
        for input in [
            r#"{"content":[{"type":"text","text":"\uD800"}]}"#,
            r#"{"content":[{"type":"text","text":"\uD800\u0041"}]}"#,
            r#"{"content":[{"type":"text","text":"\uD800\n\uDC00"}]}"#,
            r#"{"content":[{"state":{"content":false,"status":[]},"type":"text"}]}"#,
        ] {
            assert!(
                parse(input.as_bytes(), Shape::Assistant)
                    .unwrap()
                    .media
                    .is_empty()
            );
        }
    }
}
