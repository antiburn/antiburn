//! Opaque keys for provider-issued account subjects.

use anyhow::{Context, Result};
use base64::Engine as _;
use hmac::{Hmac, Mac};
use serde_json::Value;
use sha2::Digest as _;
use sha2::Sha256;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::provider_usage::live::sources::http;
use crate::provider_usage::providers;
use crate::store::Store;

const DOMAIN: &[u8] = b"antiburn/provider-account/v1\0";
const MAX_PROVIDER_BYTES: usize = 64;
const MAX_SUBJECT_BYTES: usize = 512;
const MAX_AUTH_BYTES: u64 = 256 * 1024;
const CACHE_AGE: Duration = Duration::from_secs(600);
const MAX_CACHE_ENTRIES: usize = 16;
const MAX_AUTH_ENTRIES: usize = 64;
const MAX_NETWORK_RESOLUTIONS: usize = 2;
const IDENTITY_TIMEOUT: Duration = Duration::from_secs(3);
const OPENAI_ACCOUNT_CLAIM: &str = "https://api.openai.com/auth/chatgpt_account_id";

struct CachedAccount {
    token_digest: [u8; 32],
    provider: &'static str,
    account_key: Option<String>,
    at: Instant,
}

static CACHE: OnceLock<Mutex<Vec<CachedAccount>>> = OnceLock::new();

pub(crate) fn clear_cache() {
    if let Some(cache) = CACHE.get() {
        cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clear();
    }
}

pub fn opaque_key(store: &Store, provider: &str, subject: &str) -> Result<Option<String>> {
    let provider = provider.trim();
    let subject = subject.trim();
    if provider.is_empty()
        || subject.is_empty()
        || provider.len() > MAX_PROVIDER_BYTES
        || subject.len() > MAX_SUBJECT_BYTES
    {
        return Ok(None);
    }

    let secret = store.provider_account_secret()?;
    let mut mac = Hmac::<Sha256>::new_from_slice(&secret).context("invalid account key secret")?;
    mac.update(DOMAIN);
    mac.update(&(provider.len() as u32).to_be_bytes());
    mac.update(provider.as_bytes());
    mac.update(&(subject.len() as u32).to_be_bytes());
    mac.update(subject.as_bytes());
    Ok(Some(hex(&mac.finalize().into_bytes())))
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(DIGITS[usize::from(byte >> 4)] as char);
        output.push(DIGITS[usize::from(byte & 0x0f)] as char);
    }
    output
}

pub fn observe_tool_accounts(store: &Store, provider: &'static str, observed_at_epoch: i64) {
    let mut remaining_entries = MAX_AUTH_ENTRIES;
    let mut network_budget = MAX_NETWORK_RESOLUTIONS;
    for (agent, path) in auth_paths() {
        let Some(auth) = read_auth(&path) else {
            continue;
        };
        let Some(entries) = auth.as_object() else {
            continue;
        };
        for (name, credential) in entries {
            if provider_for_auth_name(name) != Some(provider)
                || credential.get("type").and_then(Value::as_str) != Some("oauth")
            {
                continue;
            }
            if remaining_entries == 0 {
                return;
            }
            remaining_entries -= 1;
            let account_key = resolve_credential(store, provider, credential, &mut network_budget);
            tracing::debug!(
                event = "tool_provider_account_resolution",
                agent,
                provider,
                assigned = account_key.is_some()
            );
            if let Some(account_key) = account_key
                && let Err(error) = store.observe_provider_account(
                    agent,
                    provider,
                    &account_key,
                    observed_at_epoch,
                    "tool_oauth",
                )
            {
                tracing::warn!(
                    event = "tool_provider_account_observation_failed",
                    agent,
                    provider,
                    error = %error
                );
            }
        }
    }
}

fn auth_paths() -> Vec<(&'static str, PathBuf)> {
    let mut paths = Vec::new();
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let pi = std::env::var_os("PI_AGENT_DIR")
        .map(PathBuf::from)
        .or_else(|| home.as_ref().map(|home| home.join(".pi/agent")));
    if let Some(pi) = pi {
        paths.push(("pi", pi.join("auth.json")));
    }
    let opencode = std::env::var_os("OPENCODE_DATA_DIR")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("XDG_DATA_HOME")
                .map(PathBuf::from)
                .map(|path| path.join("opencode"))
        })
        .or_else(|| home.map(|home| home.join(".local/share/opencode")));
    if let Some(opencode) = opencode {
        paths.push(("opencode", opencode.join("auth.json")));
    }
    paths
}

fn read_auth(path: &Path) -> Option<Value> {
    let metadata = std::fs::symlink_metadata(path).ok()?;
    if !metadata.file_type().is_file() || metadata.len() > MAX_AUTH_BYTES {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    if bytes.len() as u64 > MAX_AUTH_BYTES {
        return None;
    }
    serde_json::from_slice(&bytes).ok()
}

fn provider_for_auth_name(name: &str) -> Option<&'static str> {
    match name {
        "anthropic" => Some(providers::ANTHROPIC),
        "openai" | "openai-codex" => Some(providers::OPENAI),
        "google" | "google-gemini-cli" | "google-antigravity" => Some(providers::GOOGLE),
        _ => None,
    }
}

fn resolve_credential(
    store: &Store,
    provider: &'static str,
    credential: &Value,
    network_budget: &mut usize,
) -> Option<String> {
    if provider == providers::OPENAI
        && let Some(subject) = credential
            .get("accountId")
            .or_else(|| credential.get("account_id"))
            .and_then(Value::as_str)
            .and_then(bounded_subject)
    {
        return opaque_key(store, provider, subject).ok().flatten();
    }
    let access = credential
        .get("access")
        .or_else(|| credential.get("accessToken"))
        .or_else(|| credential.get("access_token"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|token| !token.is_empty() && token.len() <= 32 * 1024)?;
    let digest: [u8; 32] = Sha256::digest(access.as_bytes()).into();
    if let Some(cached) = cached_key(provider, &digest) {
        return cached;
    }
    if provider != providers::OPENAI {
        *network_budget = network_budget.checked_sub(1)?;
    }
    let subject = match provider {
        providers::OPENAI => jwt_claim(access, OPENAI_ACCOUNT_CLAIM),
        providers::ANTHROPIC => fetch_subject(
            "https://api.anthropic.com/api/oauth/profile",
            access,
            &["account", "uuid"],
            true,
        ),
        providers::GOOGLE => fetch_subject(
            "https://openidconnect.googleapis.com/v1/userinfo",
            access,
            &["sub"],
            false,
        ),
        _ => None,
    };
    let key = subject.and_then(|subject| opaque_key(store, provider, &subject).ok().flatten());
    cache_key(provider, digest, key.clone());
    key
}

fn bounded_subject(subject: &str) -> Option<&str> {
    let subject = subject.trim();
    (!subject.is_empty() && subject.len() <= MAX_SUBJECT_BYTES).then_some(subject)
}

fn jwt_claim(token: &str, claim: &str) -> Option<String> {
    let payload = token.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .ok()?;
    let value: Value = serde_json::from_slice(&bytes).ok()?;
    value
        .get(claim)
        .and_then(Value::as_str)
        .and_then(bounded_subject)
        .map(str::to_owned)
}

fn fetch_subject(endpoint: &str, access: &str, path: &[&str], anthropic: bool) -> Option<String> {
    let mut request = identity_client()
        .get(endpoint)
        .bearer_auth(access)
        .header(reqwest::header::ACCEPT, "application/json");
    if anthropic {
        request = request.header("anthropic-beta", "oauth-2025-04-20");
    }
    let response = request.send().ok()?;
    if http::status_error(response.status()).is_some() {
        return None;
    }
    let body = http::read_capped_body(response).ok()?;
    let root: Value = serde_json::from_str(&body).ok()?;
    let mut value = &root;
    for field in path {
        value = value.get(*field)?;
    }
    value.as_str().and_then(bounded_subject).map(str::to_owned)
}

fn identity_client() -> &'static reqwest::blocking::Client {
    static CLIENT: OnceLock<reqwest::blocking::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
        reqwest::blocking::Client::builder()
            .timeout(IDENTITY_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("the identity client uses no custom TLS material")
    })
}

fn cached_key(provider: &'static str, digest: &[u8; 32]) -> Option<Option<String>> {
    let mut cache = CACHE
        .get_or_init(|| Mutex::new(Vec::new()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    cache.retain(|entry| entry.at.elapsed() <= CACHE_AGE);
    cache
        .iter()
        .find(|entry| entry.provider == provider && &entry.token_digest == digest)
        .map(|entry| entry.account_key.clone())
}

fn cache_key(provider: &'static str, token_digest: [u8; 32], account_key: Option<String>) {
    let mut cache = CACHE
        .get_or_init(|| Mutex::new(Vec::new()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if cache.len() == MAX_CACHE_ENTRIES {
        cache.remove(0);
    }
    cache.push(CachedAccount {
        token_digest,
        provider,
        account_key,
        at: Instant::now(),
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_stable_per_install_and_scoped_to_the_provider() {
        let first = Store::open_in_memory(std::path::Path::new("/tmp/account-key-first")).unwrap();
        let second =
            Store::open_in_memory(std::path::Path::new("/tmp/account-key-second")).unwrap();
        let openai = opaque_key(&first, "openai", "subject-a").unwrap().unwrap();

        assert_eq!(
            opaque_key(&first, "openai", "subject-a").unwrap().unwrap(),
            openai
        );
        assert_ne!(
            opaque_key(&first, "google", "subject-a").unwrap().unwrap(),
            openai
        );
        assert_ne!(
            opaque_key(&second, "openai", "subject-a").unwrap().unwrap(),
            openai
        );
        assert_eq!(openai.len(), 64);
    }

    #[test]
    fn invalid_subjects_do_not_create_keys() {
        let store =
            Store::open_in_memory(std::path::Path::new("/tmp/account-key-invalid")).unwrap();
        assert_eq!(opaque_key(&store, "openai", "  ").unwrap(), None);
        assert_eq!(
            opaque_key(&store, "openai", &"a".repeat(513)).unwrap(),
            None
        );
    }

    #[test]
    fn tool_auth_names_are_exact_and_api_keys_are_not_oauth_accounts() {
        assert_eq!(
            provider_for_auth_name("openai-codex"),
            Some(providers::OPENAI)
        );
        assert_eq!(
            provider_for_auth_name("google-antigravity"),
            Some(providers::GOOGLE)
        );
        assert_eq!(provider_for_auth_name("private-openai-proxy"), None);

        let api_key: Value = serde_json::json!({ "type": "api_key", "key": "secret" });
        assert_ne!(api_key.get("type").and_then(Value::as_str), Some("oauth"));
    }

    #[test]
    fn openai_subject_comes_from_the_chatgpt_account_claim() {
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(serde_json::json!({ (OPENAI_ACCOUNT_CLAIM): "account-id" }).to_string());
        let token = format!("header.{payload}.signature");
        assert_eq!(
            jwt_claim(&token, OPENAI_ACCOUNT_CLAIM).as_deref(),
            Some("account-id")
        );
        assert_eq!(jwt_claim(&token, "email"), None);
    }

    #[test]
    fn network_resolution_stops_when_the_budget_is_empty() {
        let store = Store::open_in_memory(std::path::Path::new("/tmp/account-key-budget")).unwrap();
        let google = serde_json::json!({"access": "synthetic-google-token"});
        let mut budget = 0;

        assert_eq!(
            resolve_credential(&store, providers::GOOGLE, &google, &mut budget),
            None
        );
        assert_eq!(budget, 0);

        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(serde_json::json!({ (OPENAI_ACCOUNT_CLAIM): "account-id" }).to_string());
        let openai = serde_json::json!({"access": format!("header.{payload}.signature")});
        assert!(resolve_credential(&store, providers::OPENAI, &openai, &mut budget).is_some());
        assert_eq!(budget, 0);
    }
}
