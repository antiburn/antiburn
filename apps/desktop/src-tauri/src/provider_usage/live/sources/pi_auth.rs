//! Read Pi's OAuth store as a read-only credential carrier.
//!
//! Pi stores provider credentials in `~/.pi/agent/auth.json`. This module
//! reads only OAuth entries for the providers that live usage supports.
//! It never refreshes a token or writes the file.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

/// Cap the read because this file is a small credential store.
const MAX_AUTH_BYTES: u64 = 256 * 1024;

/// Pi's Anthropic entry key.
pub const ANTHROPIC_KEY: &str = "anthropic";
/// Pi's Codex entry key.
pub const CODEX_KEY: &str = "openai-codex";

/// OAuth data read from one Pi provider entry.
pub struct PiOauth {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at_ms: i64,
    pub account_id: Option<String>,
}

/// Pi's auth store path.
pub fn default_auth_path() -> Option<PathBuf> {
    Some(antiburn_local::paths::home_dir()?.join(".pi/agent/auth.json"))
}

/// Read a live OAuth entry for `provider_key`.
pub fn read_entry(path: &Path, provider_key: &str) -> Option<PiOauth> {
    let metadata = fs::metadata(path).ok()?;
    if metadata.len() > MAX_AUTH_BYTES {
        return None;
    }
    let contents = fs::read_to_string(path).ok()?;
    parse_entry(&contents, provider_key)
}

fn parse_entry(contents: &str, provider_key: &str) -> Option<PiOauth> {
    let value: Value = serde_json::from_str(contents).ok()?;
    let entry = value.get(provider_key)?;
    if entry.get("type").and_then(Value::as_str) != Some("oauth") {
        return None;
    }
    let access_token = entry.get("access")?.as_str()?.to_owned();
    let refresh_token = entry.get("refresh")?.as_str()?.to_owned();
    let expires_at_ms = entry.get("expires")?.as_i64()?;
    if access_token.is_empty() || refresh_token.is_empty() || expires_at_ms <= 0 {
        return None;
    }
    let account_id = entry
        .get("accountId")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    Some(PiOauth {
        access_token,
        refresh_token,
        expires_at_ms,
        account_id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const STORE: &str = r#"{
      "anthropic": {"type": "oauth", "access": "synthetic-anthropic-access",
        "refresh": "synthetic-refresh", "expires": 1800000000000},
      "openai-codex": {"type": "oauth", "access": "synthetic-codex-access",
        "refresh": "synthetic-refresh", "expires": 1800000000000,
        "accountId": "synthetic-account"},
      "openrouter": {"type": "api-key", "key": "synthetic-key"}
    }"#;

    #[test]
    fn reads_oauth_entries_and_optional_account_id() {
        let anthropic = parse_entry(STORE, ANTHROPIC_KEY).expect("anthropic entry");
        assert_eq!(anthropic.access_token, "synthetic-anthropic-access");
        assert_eq!(anthropic.expires_at_ms, 1_800_000_000_000);
        assert_eq!(anthropic.account_id, None);
        let codex = parse_entry(STORE, CODEX_KEY).expect("codex entry");
        assert_eq!(codex.account_id.as_deref(), Some("synthetic-account"));
    }

    #[test]
    fn rejects_non_oauth_missing_and_tombstoned_entries() {
        assert!(parse_entry(STORE, "openrouter").is_none());
        assert!(parse_entry(STORE, "missing").is_none());
        for entry in [
            r#"{"anthropic":{"type":"oauth","access":"","refresh":"r","expires":1800000000000}}"#,
            r#"{"anthropic":{"type":"oauth","access":"a","refresh":"","expires":1800000000000}}"#,
            r#"{"anthropic":{"type":"oauth","access":"a","refresh":"r","expires":0}}"#,
        ] {
            assert!(parse_entry(entry, ANTHROPIC_KEY).is_none());
        }
    }

    #[test]
    fn enforces_size_cap() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("auth.json");
        fs::write(
            &path,
            format!(
                r#"{{"padding":"{}"}}"#,
                "x".repeat((MAX_AUTH_BYTES + 1) as usize)
            ),
        )
        .expect("write");
        assert!(read_entry(&path, ANTHROPIC_KEY).is_none());
    }
}
