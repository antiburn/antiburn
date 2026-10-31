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

impl PiOauth {
    /// Whether the access token has not yet passed its stated expiry.
    pub fn is_live(&self, now: time::OffsetDateTime) -> bool {
        !self.access_token.trim().is_empty()
            && i128::from(self.expires_at_ms) > now.unix_timestamp_nanos() / 1_000_000
    }
}

/// Set to `1` to make live usage ignore Pi's auth store. Debug builds only.
/// A tester who uses Pi can then see what a reader without Pi sees, for
/// example the Claude Desktop-only state.
const IGNORE_ENV: &str = "ANTIBURN_IGNORE_PI_AUTH";

/// Pi's auth store path, or `None` when a debug build ignores Pi.
pub fn default_auth_path() -> Option<PathBuf> {
    if ignore_requested(cfg!(debug_assertions), std::env::var_os(IGNORE_ENV)) {
        ::tracing::info!(event = "pi_auth_ignored");
        return None;
    }
    Some(antiburn_local::paths::home_dir()?.join(".pi/agent/auth.json"))
}

/// Whether to ignore Pi: only in a debug build, and only for the value `1`.
fn ignore_requested(debug_build: bool, value: Option<std::ffi::OsString>) -> bool {
    debug_build && value.is_some_and(|value| value == "1")
}

/// The provider keys the store holds, and nothing under them.
///
/// Detection asks this to learn *which* providers to ask Pi about. The
/// values are never inspected: Pi's own `auth check --no-refresh` answers
/// whether an entry is usable, so no token passes through this process.
pub fn provider_keys(path: &Path) -> Option<std::collections::BTreeSet<String>> {
    let metadata = fs::metadata(path).ok()?;
    if metadata.len() > MAX_AUTH_BYTES {
        return None;
    }
    let contents = fs::read_to_string(path).ok()?;
    let value: Value = serde_json::from_str(&contents).ok()?;
    Some(value.as_object()?.keys().cloned().collect())
}

/// Read a complete OAuth entry for `provider_key`, including expired entries.
pub fn read_entry(path: &Path, provider_key: &str) -> Option<PiOauth> {
    read_entry_checked(path, provider_key).ok().flatten()
}

/// Keep an unreadable store separate from an absent provider login.
pub fn read_entry_checked(path: &Path, provider_key: &str) -> std::io::Result<Option<PiOauth>> {
    Ok(read_raw_entry_checked(path, provider_key)?.filter(|entry| {
        !entry.access_token.is_empty() && !entry.refresh_token.is_empty() && entry.expires_at_ms > 0
    }))
}

/// Read an OAuth login even when its credential fields are incomplete.
/// Callers must check usability before they use the token.
pub fn read_raw_entry_checked(path: &Path, provider_key: &str) -> std::io::Result<Option<PiOauth>> {
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    if metadata.len() > MAX_AUTH_BYTES {
        return Err(std::io::ErrorKind::InvalidData.into());
    }
    let contents = fs::read_to_string(path)?;
    let value: Value =
        serde_json::from_str(&contents).map_err(|_| std::io::ErrorKind::InvalidData)?;
    Ok(parse_entry(&value, provider_key))
}

fn parse_entry(value: &Value, provider_key: &str) -> Option<PiOauth> {
    let entry = value.get(provider_key)?;
    if entry.get("type").and_then(Value::as_str) != Some("oauth") {
        return None;
    }
    let access_token = entry
        .get("access")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let refresh_token = entry
        .get("refresh")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let expires_at_ms = entry
        .get("expires")
        .and_then(Value::as_i64)
        .unwrap_or_default();
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

    fn parse_entry(contents: &str, provider_key: &str) -> Option<PiOauth> {
        super::parse_entry(&serde_json::from_str::<Value>(contents).ok()?, provider_key)
    }

    #[test]
    fn only_a_debug_build_with_the_value_one_ignores_pi() {
        assert!(ignore_requested(true, Some("1".into())));
        assert!(!ignore_requested(false, Some("1".into())));
        assert!(!ignore_requested(true, Some("0".into())));
        assert!(!ignore_requested(true, Some("true".into())));
        assert!(!ignore_requested(true, None));
    }

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
    fn raw_parser_keeps_incomplete_logins_but_rejects_other_shapes() {
        assert!(parse_entry(STORE, "openrouter").is_none());
        assert!(parse_entry(STORE, "missing").is_none());
        for entry in [
            r#"{"anthropic":{"type":"oauth","access":"","refresh":"r","expires":1800000000000}}"#,
            r#"{"anthropic":{"type":"oauth","access":"a","refresh":"","expires":1800000000000}}"#,
            r#"{"anthropic":{"type":"oauth","access":"a","refresh":"r","expires":0}}"#,
        ] {
            assert!(parse_entry(entry, ANTHROPIC_KEY).is_some());
        }
    }

    #[test]
    fn liveness_compares_the_stated_expiry_to_now() {
        let entry = parse_entry(STORE, ANTHROPIC_KEY).expect("anthropic entry");
        let before = time::OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap();
        let after = time::OffsetDateTime::from_unix_timestamp(1_900_000_000).unwrap();
        assert!(entry.is_live(before));
        assert!(!entry.is_live(after));
    }

    #[test]
    fn strict_readers_reject_incomplete_entries_for_every_provider() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("auth.json");
        for provider in [ANTHROPIC_KEY, CODEX_KEY] {
            for entry in [
                r#"{"type":"oauth"}"#,
                r#"{"type":"oauth","access":"","refresh":"r","expires":1}"#,
                r#"{"type":"oauth","access":"a","refresh":"","expires":1}"#,
                r#"{"type":"oauth","access":"a","refresh":"r","expires":0}"#,
                r#"{"type":"oauth","access":1,"refresh":"r","expires":1}"#,
                r#"{"type":"oauth","access":"a","refresh":1,"expires":1}"#,
                r#"{"type":"oauth","access":"a","refresh":"r","expires":"1"}"#,
            ] {
                fs::write(&path, format!(r#"{{"{provider}":{entry}}}"#)).unwrap();
                assert!(read_entry(&path, provider).is_none());
                assert!(read_entry_checked(&path, provider).unwrap().is_none());
                assert!(read_raw_entry_checked(&path, provider).unwrap().is_some());
            }
            fs::write(
                &path,
                format!(
                    r#"{{"{provider}":{{"type":"oauth","access":"a","refresh":"r","expires":1}}}}"#
                ),
            )
            .unwrap();
            assert!(read_entry_checked(&path, provider).unwrap().is_some());
        }
    }

    #[test]
    fn checked_readers_distinguish_missing_login_from_invalid_json() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("auth.json");
        for reader in [read_entry_checked, read_raw_entry_checked] {
            assert!(reader(&path, ANTHROPIC_KEY).unwrap().is_none());
            fs::write(&path, "{}").unwrap();
            assert!(reader(&path, ANTHROPIC_KEY).unwrap().is_none());
            fs::write(&path, "{").unwrap();
            assert_eq!(
                reader(&path, ANTHROPIC_KEY).err().unwrap().kind(),
                std::io::ErrorKind::InvalidData
            );
            fs::remove_file(&path).unwrap();
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
