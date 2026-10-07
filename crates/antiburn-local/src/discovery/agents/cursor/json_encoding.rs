pub(super) fn looks_like_jsonish(text: &str) -> bool {
    let trimmed = text.trim();
    trimmed.starts_with('{') || trimmed.starts_with('[')
}

/// Cursor stores `~/.cursor/chats/*/store.db#meta.value` as hex-encoded JSON
/// in a TEXT column. Check the encoding before allocating the output buffer.
pub(super) fn decode_hex_jsonish(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.len() < 4 || !trimmed.len().is_multiple_of(2) {
        return None;
    }
    let bytes = trimmed.as_bytes();
    let head = hex_byte(bytes[0], bytes[1])?;
    if head != b'{' && head != b'[' {
        return None;
    }
    if !bytes[2..].iter().all(u8::is_ascii_hexdigit) {
        return None;
    }
    let mut out = Vec::with_capacity(trimmed.len() / 2);
    out.push(head);
    for pair in bytes[2..].chunks_exact(2) {
        out.push(hex_byte(pair[0], pair[1])?);
    }
    String::from_utf8(out).ok()
}

fn hex_byte(hi: u8, lo: u8) -> Option<u8> {
    let h = (hi as char).to_digit(16)?;
    let l = (lo as char).to_digit(16)?;
    Some(((h << 4) | l) as u8)
}
