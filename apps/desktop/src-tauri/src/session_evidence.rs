//! Deterministic retained-session content search and evidence retrieval.

pub(crate) mod deep;

#[cfg(test)]
mod tests;

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use tauri::Manager as _;

use crate::UiReadStore;
use crate::session_search::SessionSearchEntry;
use crate::store::{
    RetainedContentRow, SessionSearchResult, StoredEvidenceReference, iso_from_epoch,
};

pub(super) const CACHE_BYTE_LIMIT: usize = 32 * 1024 * 1024;
pub(super) const EXCERPT_CHARS: usize = 420;
pub(super) const MAX_SAFE_JS_INTEGER: u64 = 9_007_199_254_740_991;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub(crate) enum EvidenceKind {
    User,
    Assistant,
    Thinking,
    ToolInput,
    ToolResult,
    ToolError,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct EvidenceReference {
    pub key: String,
    pub environment_key: String,
    pub agent: String,
    pub session_id: String,
    pub source_generation: i64,
    pub published_fence: i64,
    pub source_key: String,
    pub thread_id: String,
    pub scope: String,
    pub turn_row_id: i64,
    pub turn_index: i64,
    pub part_index: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub match_start: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub match_end: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub json_path: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EvidenceCoverage {
    pub(super) state: &'static str,
    pub(super) inspected_bytes: usize,
    pub(super) byte_limit: usize,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SessionEvidenceHit {
    pub(super) session: SessionSearchEntry,
    pub(super) reference: EvidenceReference,
    pub(super) excerpt: String,
    pub(super) kind: EvidenceKind,
    pub(super) score: u64,
    pub(super) retrieval_rank: usize,
    pub(super) coverage: EvidenceCoverage,
    pub(super) truncated: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EvidenceContextItem {
    reference: EvidenceReference,
    kind: EvidenceKind,
    text: String,
    truncated: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EvidenceContextResponse {
    available: bool,
    reason: Option<&'static str>,
    session: Option<SessionSearchEntry>,
    previous: Option<EvidenceContextItem>,
    #[serde(rename = "match")]
    matched: Option<EvidenceContextItem>,
    next: Option<EvidenceContextItem>,
}

#[tauri::command]
pub(crate) async fn fetch_session_evidence(
    app: tauri::AppHandle,
    reference: EvidenceReference,
) -> Result<EvidenceContextResponse, String> {
    let Some(stored) = validate_reference(&reference) else {
        return Ok(unavailable_context("unavailable"));
    };
    let reader = app.state::<UiReadStore>().0.clone();
    let work_reference = stored.clone();
    let found = tauri::async_runtime::spawn_blocking(move || {
        reader.fetch_published_session_context(&work_reference)
    })
    .await
    .map_err(|_| "unavailable".to_owned())?
    .map_err(|_| "search_failed".to_owned())?;
    let Some(context) = found else {
        return Ok(unavailable_context("stale"));
    };
    if reference_key(&stored, &context.matched.reference_bytes) != reference.key {
        return Ok(unavailable_context("stale"));
    }
    let Some(matched) = context_item(&reference, &context.matched) else {
        return Ok(unavailable_context("stale"));
    };
    Ok(EvidenceContextResponse {
        available: true,
        reason: None,
        session: Some(session_entry(context.session)),
        previous: context
            .previous
            .as_ref()
            .and_then(|row| neighboring_context_item(&reference, row)),
        matched: Some(matched),
        next: context
            .next
            .as_ref()
            .and_then(|row| neighboring_context_item(&reference, row)),
    })
}

fn unavailable_context(reason: &'static str) -> EvidenceContextResponse {
    EvidenceContextResponse {
        available: false,
        reason: Some(reason),
        session: None,
        previous: None,
        matched: None,
        next: None,
    }
}

pub(super) fn lowercase_with_offsets(text: &str) -> (String, Vec<usize>) {
    let mut lowercase = String::with_capacity(text.len());
    let mut original_offsets = Vec::with_capacity(text.len() + 1);
    for (original_offset, character) in text.char_indices() {
        lowercase.extend(character.to_lowercase());
        original_offsets.resize(lowercase.len(), original_offset);
    }
    original_offsets.push(text.len());
    (lowercase, original_offsets)
}

pub(super) fn evidence_reference(
    stored: StoredEvidenceReference,
    reference_bytes: &[u8],
) -> EvidenceReference {
    EvidenceReference {
        key: reference_key(&stored, reference_bytes),
        environment_key: stored.environment_key,
        agent: stored.agent,
        session_id: stored.session_id,
        source_generation: stored.source_generation,
        published_fence: stored.published_fence,
        source_key: stored.source_key,
        thread_id: stored.thread_id,
        scope: stored.scope,
        turn_row_id: stored.turn_rowid,
        turn_index: stored.turn_index,
        part_index: stored.part_index,
        match_start: None,
        match_end: None,
        json_path: None,
    }
}

fn reference_key(reference: &StoredEvidenceReference, reference_bytes: &[u8]) -> String {
    let mut digest = Sha256::new();
    for value in [
        "session-evidence-v1",
        &reference.environment_key,
        &reference.agent,
        &reference.session_id,
        &reference.source_generation.to_string(),
        &reference.published_fence.to_string(),
        &reference.source_key,
        &reference.thread_id,
        &reference.scope,
        &reference.turn_rowid.to_string(),
        &reference.turn_index.to_string(),
        &reference.part_index.to_string(),
    ] {
        digest.update(value.as_bytes());
        digest.update([0]);
    }
    digest.update(reference_bytes);
    digest.update([0]);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest.finalize())
}

fn validate_reference(reference: &EvidenceReference) -> Option<StoredEvidenceReference> {
    if reference.key.len() != 43
        || reference.environment_key.is_empty()
        || reference.agent.is_empty()
        || reference.session_id.is_empty()
        || reference.source_generation < 0
        || reference.published_fence < 0
        || reference.source_key.is_empty()
        || reference.thread_id.is_empty()
        || !matches!(reference.scope.as_str(), "main" | "delegated")
        || reference.turn_row_id < 0
        || reference.turn_index < 0
        || reference.part_index < 0
    {
        return None;
    }
    Some(StoredEvidenceReference {
        environment_key: reference.environment_key.clone(),
        agent: reference.agent.clone(),
        session_id: reference.session_id.clone(),
        source_generation: reference.source_generation,
        published_fence: reference.published_fence,
        source_key: reference.source_key.clone(),
        thread_id: reference.thread_id.clone(),
        scope: reference.scope.clone(),
        turn_rowid: reference.turn_row_id,
        turn_index: reference.turn_index,
        part_index: reference.part_index,
    })
}

fn context_item(
    reference: &EvidenceReference,
    row: &RetainedContentRow,
) -> Option<EvidenceContextItem> {
    let kind = evidence_kind(&row.kind, &row.content)?;
    let text = if let Some(path) = reference.json_path.as_deref() {
        serde_json::from_str::<serde_json::Value>(&row.content)
            .ok()?
            .pointer(path)?
            .as_str()?
            .to_owned()
    } else {
        row.content.clone()
    };
    if let (Some(start), Some(end)) = (reference.match_start, reference.match_end)
        && (start > end || end > text.chars().count())
    {
        return None;
    }
    Some(EvidenceContextItem {
        reference: reference.clone(),
        kind,
        text,
        truncated: row.truncated,
    })
}

fn neighboring_context_item(
    base: &EvidenceReference,
    row: &RetainedContentRow,
) -> Option<EvidenceContextItem> {
    let stored = StoredEvidenceReference {
        environment_key: base.environment_key.clone(),
        agent: base.agent.clone(),
        session_id: base.session_id.clone(),
        source_generation: base.source_generation,
        published_fence: base.published_fence,
        source_key: row.source_key.clone(),
        thread_id: row.thread_id.clone(),
        scope: row.scope.clone(),
        turn_rowid: row.turn_rowid,
        turn_index: row.turn_index,
        part_index: row.part_index,
    };
    let reference = evidence_reference(stored, &row.reference_bytes);
    context_item(&reference, row)
}

fn evidence_kind(kind: &str, text: &str) -> Option<EvidenceKind> {
    Some(match kind {
        "user" => EvidenceKind::User,
        "assistant" => EvidenceKind::Assistant,
        "thinking" => EvidenceKind::Thinking,
        "tool_input" => EvidenceKind::ToolInput,
        "tool_result"
            if ["error", "failed", "failure", "exception", "denied"]
                .iter()
                .any(|marker| text.to_lowercase().contains(marker)) =>
        {
            EvidenceKind::ToolError
        }
        "tool_result" => EvidenceKind::ToolResult,
        _ => return None,
    })
}

pub(super) fn session_entry(result: SessionSearchResult) -> SessionSearchEntry {
    SessionSearchEntry {
        environment_key: result.environment_key,
        agent: result.agent,
        session_id: result.session_id,
        wsl_distro: result.wsl_distro,
        title: result.title,
        repository: result.repository,
        cwd_label: result.cwd_label,
        models: result.models,
        timestamp: iso_from_epoch(result.updated_at_epoch),
    }
}
