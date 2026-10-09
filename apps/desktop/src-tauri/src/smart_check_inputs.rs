//! Publication-bound production inputs. This module does not enroll checks.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use antiburn_local::analysis::jev::{JevCheck, JevError, JevInputSelection};
use antiburn_local::analysis::jev_evidence::{
    SessionContentEvidence, prepare_session_content, select_session_content,
};
use antiburn_local::analysis::session_scope::{SessionScopeBoundary, SessionScopeSnapshot};
use antiburn_local::analysis::{
    PublishedContent, SelectedContentQueryError, SelectedContentRequest,
};
use antiburn_local::checks::{over_exploring, scope_creep, skill_opportunities};
use sha2::{Digest, Sha256};

use crate::session_scope::ScopeLoadError;
use crate::store::{SessionKey, Store};

pub(crate) mod cache;
mod episodes;
mod inventory;
pub(crate) mod inventory_cache;
pub use episodes::{EpisodeCompletion, InvestigationSpan};
pub use inventory::{InventoryRevisionChange, InventoryRevisionObserver, SkillInputs};

const MAX_ACTIVITY_BYTES: usize = 16 * 1024 * 1024;

#[cfg(test)]
thread_local! {
    pub(crate) static INPUT_LOAD_COUNT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetectorInput {
    ScopeCreep,
    OverExploring,
    SkillOpportunities,
}

impl DetectorInput {
    fn selection(self) -> JevInputSelection {
        match self {
            Self::ScopeCreep => scope_creep::INPUT_SELECTION,
            Self::OverExploring => over_exploring::OverExploringCheck.input_selection(),
            Self::SkillOpportunities => skill_opportunities::SKILL_OPPORTUNITIES_INPUT_SELECTION,
        }
    }

    fn max_events(self) -> usize {
        match self {
            Self::ScopeCreep => 65_536,
            Self::OverExploring => 4096,
            Self::SkillOpportunities => antiburn_local::analysis::jev::MAX_SELECTED_EVIDENCE_PARTS,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputUnavailable {
    PublicationChanged,
    BoundaryMissing,
    AssemblyLimitReached,
    IncompleteEvidence,
    InvalidEventOrder,
    WrongDetector,
    UnsupportedInventoryEnvironment,
    InventoryContextMismatch,
    InventoryIncomplete,
}

#[derive(Debug)]
pub enum InputLoadError {
    Storage(anyhow::Error),
    Unavailable(InputUnavailable),
    Scope(ScopeLoadError),
    Query(SelectedContentQueryError),
    Preparation(JevError),
    Inventory(crate::agent_config::SkillSnapshotError),
    SkillUse(skill_opportunities::SkillInputError),
    Serialization(serde_json::Error),
}

impl InputLoadError {
    pub(crate) fn failure_category(&self) -> &'static str {
        match self {
            Self::Unavailable(reason) => match reason {
                InputUnavailable::PublicationChanged => "publication_changed",
                InputUnavailable::BoundaryMissing => "boundary_missing",
                InputUnavailable::AssemblyLimitReached => "input_assembly_limit",
                InputUnavailable::IncompleteEvidence => "input_evidence_incomplete",
                InputUnavailable::InvalidEventOrder => "input_event_order_invalid",
                InputUnavailable::WrongDetector => "input_detector_mismatch",
                InputUnavailable::UnsupportedInventoryEnvironment => {
                    "inventory_environment_unsupported"
                }
                InputUnavailable::InventoryContextMismatch => "inventory_context_mismatch",
                InputUnavailable::InventoryIncomplete => "inventory_incomplete",
            },
            Self::Scope(ScopeLoadError::Scope(error)) => match error {
                antiburn_local::analysis::session_scope::SessionScopeError::Missing(reason) => {
                    use antiburn_local::analysis::session_scope::ScopeMissingReason;
                    match reason {
                        ScopeMissingReason::PublicationChanged => "publication_changed",
                        ScopeMissingReason::BoundaryMissing => "scope_boundary_missing",
                        ScopeMissingReason::BranchUnresolved => "scope_branch_unresolved",
                        ScopeMissingReason::NoUserContext => "scope_task_missing",
                        ScopeMissingReason::TruncatedEvidence => "scope_evidence_truncated",
                        ScopeMissingReason::InvalidEvidence => "scope_evidence_invalid",
                        ScopeMissingReason::IncompleteSource => "scope_source_incomplete",
                        ScopeMissingReason::IncompletePaging => "scope_paging_incomplete",
                        ScopeMissingReason::UnsupportedSource => "scope_source_unsupported",
                        ScopeMissingReason::UnresolvedInfluence => "scope_influence_unresolved",
                        ScopeMissingReason::MissingCapabilities => "scope_capabilities_missing",
                        ScopeMissingReason::AssemblyLimitReached => "scope_assembly_limit",
                    }
                }
                antiburn_local::analysis::session_scope::SessionScopeError::ScopeTooLarge => {
                    "scope_context_too_large"
                }
            },
            Self::Scope(ScopeLoadError::Query(_)) | Self::Query(_) => "input_query_failed",
            Self::Preparation(JevError::RequestTokenLimitExceeded { .. }) => {
                "input_context_too_large"
            }
            Self::Preparation(JevError::RequestTooLarge { .. }) => "input_request_too_large",
            Self::Preparation(error) => crate::jev::worker::error_category(error),
            Self::Inventory(_) => "skill_inventory_unavailable",
            Self::SkillUse(_) => "skill_use_invalid",
            Self::Storage(_) => "input_storage_failed",
            Self::Serialization(_) => "input_serialization_failed",
        }
    }

    pub(crate) fn is_stale(&self) -> bool {
        matches!(self, Self::Unavailable(InputUnavailable::PublicationChanged)
            | Self::Scope(ScopeLoadError::Scope(antiburn_local::analysis::session_scope::SessionScopeError::Missing(
                antiburn_local::analysis::session_scope::ScopeMissingReason::PublicationChanged)))
            | Self::Query(SelectedContentQueryError::StaleCursor)
            | Self::Scope(ScopeLoadError::Query(SelectedContentQueryError::StaleCursor)))
    }
}

#[derive(Debug, Clone)]
pub struct SmartCheckInputSnapshot {
    key: SessionKey,
    detector: DetectorInput,
    scope: Arc<SessionScopeSnapshot>,
    boundary: SessionScopeBoundary,
    content: SessionContentEvidence,
    revision: String,
    investigation_spans: Vec<InvestigationSpan>,
}

impl SmartCheckInputSnapshot {
    pub(crate) fn for_candidate(
        mut self,
        candidate: &crate::store::BurnCheckCandidate,
    ) -> Result<Self, InputLoadError> {
        if self.key != candidate.session.key
            || self.scope.publication_fence() != candidate.published_fence
            || self.scope.source_generation() != candidate.source_generation
        {
            return Err(unavailable(InputUnavailable::PublicationChanged));
        }
        let positions = candidate
            .boundary_positions
            .iter()
            .map(|(source, position)| {
                (
                    antiburn_local::analysis::ignored_instructions::sha256_hex(source.as_bytes()),
                    *position,
                )
            })
            .collect::<BTreeMap<_, _>>();
        for action in &mut self.content.actions {
            if candidate.historical {
                continue;
            }
            let enrolled = positions
                .get(&action.reference.source_key_digest)
                .map_or_else(
                    || {
                        action.timestamp_ms.is_some_and(|time| {
                            time >= candidate.boundary_at_epoch.saturating_mul(1000)
                        })
                    },
                    |position| action.reference.turn_index > *position,
                );
            action.context_only |= !enrolled;
        }
        self.revision = digest(&serde_json::json!((
            &self.revision,
            &candidate.boundary_positions,
            candidate.boundary_at_epoch,
            candidate.historical
        )))?;
        Ok(self)
    }

    pub fn scope(&self) -> &SessionScopeSnapshot {
        &self.scope
    }

    pub fn boundary(&self) -> &SessionScopeBoundary {
        &self.boundary
    }

    pub fn content(&self) -> &SessionContentEvidence {
        &self.content
    }

    pub fn input_revision(&self) -> &str {
        &self.revision
    }

    pub fn investigation_spans(&self) -> &[InvestigationSpan] {
        &self.investigation_spans
    }

    pub fn scope_creep_input(
        &self,
        ignored_instruction_work_ids: BTreeSet<String>,
    ) -> Result<scope_creep::ScopeCreepInput, InputLoadError> {
        self.require_detector(DetectorInput::ScopeCreep)?;
        Ok(scope_creep::ScopeCreepInput {
            scope: self.scope.clone(),
            content: self.content.clone(),
            boundary: self.boundary.clone(),
            source_generation: self.scope.source_generation(),
            ignored_instruction_work_ids,
        })
    }

    pub fn over_exploring_input(
        &self,
    ) -> Result<over_exploring::OverExploringInput, InputLoadError> {
        self.require_detector(DetectorInput::OverExploring)?;
        let spans: Vec<_> = self
            .investigation_spans
            .iter()
            .map(|item| item.span.clone())
            .collect();
        over_exploring::build_episodes(&self.content, &self.scope, &spans)
            .map_err(InputLoadError::Preparation)
    }

    fn require_detector(&self, detector: DetectorInput) -> Result<(), InputLoadError> {
        if self.detector == detector {
            Ok(())
        } else {
            Err(unavailable(InputUnavailable::WrongDetector))
        }
    }
}

impl Store {
    /// Read full scope and detector-selected activity from source start at one publication.
    pub fn load_smart_check_inputs(
        &self,
        key: &SessionKey,
        publication_fence: i64,
        source_generation: i64,
        detector: DetectorInput,
    ) -> Result<SmartCheckInputSnapshot, InputLoadError> {
        let started = std::time::Instant::now();
        #[cfg(test)]
        INPUT_LOAD_COUNT.set(INPUT_LOAD_COUNT.get() + 1);
        let request = self
            .session_scope_request(key, publication_fence, source_generation)
            .map_err(InputLoadError::Scope)?;
        let boundary = request.boundary().clone();
        let source_format = request.source_format();
        let scope = Arc::new(
            self.load_session_scope(key, request)
                .map_err(InputLoadError::Scope)?,
        );
        let (published, limitations) = self.collect_smart_check_activity(
            key,
            publication_fence,
            source_generation,
            &boundary,
            detector,
        )?;
        // Normalize once so a result on another page can bind its request.
        let identity =
            serde_json::to_string(&(key.environment_key.as_str(), &key.agent, &key.session_id))
                .map_err(InputLoadError::Serialization)?;
        let mut prepared = prepare_session_content(&identity, source_format, published, Vec::new());
        prepared.limitations.extend(limitations);
        prepared.limitations.extend(
            scope
                .limitations()
                .iter()
                .map(|limitation| format!("scope_{limitation:?}")),
        );
        let scope_user_ids = scope
            .occurrences()
            .iter()
            .filter(|occurrence| {
                occurrence.authority
                    == antiburn_local::analysis::session_scope::ScopeAuthority::User
            })
            .map(|occurrence| occurrence.reference.id.as_str())
            .collect::<BTreeSet<_>>();
        // Only source-bound scope text can supply user authority.
        prepared.actions.retain(|action| {
            !(action.kind == "user"
                && action.authority == "user"
                && !scope_user_ids.contains(action.reference.id.as_str()))
        });
        prepared
            .limitations
            .extend(tool_result_limitations(&prepared.actions));
        prepared.complete &= prepared.limitations.is_empty();
        let mut content = select_session_content(&prepared, detector.selection());
        let initial_limitations = prepared.limitations.len();
        let investigation_spans = if detector == DetectorInput::OverExploring {
            episodes::investigation_spans(&content, &scope)?
        } else {
            Vec::new()
        };
        if investigation_spans.iter().any(|span| {
            !scope.occurrences().iter().any(|occurrence| {
                occurrence.authority
                    == antiburn_local::analysis::session_scope::ScopeAuthority::User
                    && occurrence.reference.id == span.span.first_event_id
            })
        }) {
            prepared
                .limitations
                .push("investigation_task_context_unavailable".to_owned());
            prepared.complete = false;
        }
        if investigation_spans
            .iter()
            .any(|span| span.completion == EpisodeCompletion::Unknown)
        {
            prepared.limitations.push("open_investigation".to_owned());
            prepared.complete = false;
        }
        if prepared.limitations.len() != initial_limitations {
            content = select_session_content(&prepared, detector.selection());
        }
        let revision = digest(&serde_json::json!({
            "scope": &scope, "activity": content.selected_input_digest,
            "generation": source_generation, "boundary": boundary.content_scope()
                .map_err(|error| InputLoadError::Scope(ScopeLoadError::Scope(error)))?,
        }))?;
        self.validate_smart_check_publication(
            key,
            publication_fence,
            source_generation,
            &boundary,
        )?;
        let snapshot = SmartCheckInputSnapshot {
            key: key.clone(),
            detector,
            scope,
            boundary,
            content,
            revision,
            investigation_spans,
        };
        ::tracing::debug!(
            event = "smart_check_input_loaded",
            detector = ?detector,
            scope_occurrences = snapshot.scope.occurrences().len(),
            selected_actions = snapshot.content.actions.len(),
            limitations = snapshot.content.limitations.len(),
            elapsed_ms = started.elapsed().as_millis(),
        );
        Ok(snapshot)
    }

    fn collect_smart_check_activity(
        &self,
        key: &SessionKey,
        publication_fence: i64,
        source_generation: i64,
        boundary: &SessionScopeBoundary,
        detector: DetectorInput,
    ) -> Result<(PublishedContent, Vec<String>), InputLoadError> {
        let content_scope = boundary
            .content_scope()
            .map_err(|error| InputLoadError::Scope(ScopeLoadError::Scope(error)))?;
        let positions = BTreeMap::new();
        let mut cursor = None;
        let mut collected = PublishedContent {
            publication_fence,
            source_generation: Some(source_generation),
            ..Default::default()
        };
        let mut bytes = 0usize;
        let mut previous = None;
        let mut limitations = Vec::new();
        'pages: loop {
            let page = self
                .published_turn_content_keyset_scoped(
                    key,
                    SelectedContentRequest {
                        source_generation,
                        after_ms: None,
                        source_positions: &positions,
                        selection: detector.selection(),
                        cursor: cursor.as_ref(),
                    },
                    &content_scope,
                )
                .map_err(InputLoadError::Query)?
                .ok_or_else(|| unavailable(InputUnavailable::PublicationChanged))?;
            if page.page.content.publication_fence != publication_fence {
                return Err(unavailable(InputUnavailable::PublicationChanged));
            }
            if page.page.content.source_generation != Some(source_generation) {
                return Err(unavailable(InputUnavailable::PublicationChanged));
            }
            if page.boundary.is_none() {
                return Err(unavailable(InputUnavailable::BoundaryMissing));
            }
            let coverage = page.page.content.coverage;
            merge_page_coverage(
                &mut collected.coverage,
                coverage,
                page.page.next_cursor.is_some(),
            );
            for part in page.page.content.parts {
                if part.part.kind.as_str() == "thinking" || part.role == "thinking" {
                    continue;
                }
                let position = (part.turn_index, part.part_index);
                if part.source_key != boundary.source_key
                    || part.thread_id != boundary.thread_id
                    || part.scope != "main"
                    || part.context_only
                    || !part.stable_event_identity
                    || (part.uuid.is_none() && part.message_id.is_none())
                {
                    limitations.push("invalid_selected_event_identity".to_owned());
                    continue;
                }
                if previous.is_some_and(|last| last >= position) {
                    limitations.push("invalid_selected_event_order".to_owned());
                    continue;
                }
                previous = Some(position);
                if part.part.kind.as_str() == "tool_input"
                    && part
                        .part
                        .normalized_fields
                        .as_ref()
                        .is_some_and(|fields| fields.malformed || fields.values.is_empty())
                {
                    limitations.push("malformed_selected_tool_input".to_owned());
                    continue;
                }
                let part_bytes = retained_part_bytes(&part)?;
                if collected.parts.len() == detector.max_events() {
                    collected.coverage.parts_capped = true;
                    collected.coverage.more_parts = true;
                    break 'pages;
                }
                if part_bytes > MAX_ACTIVITY_BYTES.saturating_sub(bytes) {
                    collected.coverage.bytes_capped = true;
                    collected.coverage.oversized_parts =
                        collected.coverage.oversized_parts.saturating_add(1);
                    continue;
                }
                bytes += part_bytes;
                collected.parts.push(part);
            }
            if page.page.next_cursor.is_some() && page.page.next_cursor == cursor {
                return Err(unavailable(InputUnavailable::InvalidEventOrder));
            }
            cursor = page.page.next_cursor;
            if cursor.is_none() {
                break;
            }
        }
        limitations.sort();
        limitations.dedup();
        Ok((collected, limitations))
    }

    fn validate_smart_check_publication(
        &self,
        key: &SessionKey,
        publication_fence: i64,
        source_generation: i64,
        boundary: &SessionScopeBoundary,
    ) -> Result<(), InputLoadError> {
        let current = self
            .session_scope_request(key, publication_fence, source_generation)
            .map_err(InputLoadError::Scope)?;
        if current.boundary() != boundary {
            return Err(unavailable(InputUnavailable::PublicationChanged));
        }
        Ok(())
    }
}

fn merge_page_coverage(
    collected: &mut antiburn_local::analysis::ContentQueryCoverage,
    page: antiburn_local::analysis::ContentQueryCoverage,
    has_next: bool,
) {
    collected.context_capped |= page.context_capped;
    collected.oversized_parts = collected
        .oversized_parts
        .saturating_add(page.oversized_parts);
    collected.stored_truncated_parts = collected
        .stored_truncated_parts
        .saturating_add(page.stored_truncated_parts);
    // A drained page boundary does not remove observations.
    if !has_next {
        collected.parts_capped |= page.parts_capped || page.more_parts;
        collected.bytes_capped |= page.bytes_capped;
        collected.more_parts |= page.more_parts;
    }
}

fn tool_result_limitations(
    actions: &[antiburn_local::analysis::jev_evidence::ContentAction],
) -> Vec<String> {
    let mut calls = BTreeMap::new();
    for action in actions {
        if !matches!(action.kind.as_str(), "tool_input" | "tool_result") {
            continue;
        }
        let (Some(name), Some(call)) = (&action.tool_name, &action.tool_call_id) else {
            continue;
        };
        let (requests, results) = calls
            .entry((name, call))
            .or_insert((Vec::new(), Vec::new()));
        if action.kind == "tool_input" {
            requests.push(action);
        } else {
            results.push(action);
        }
    }
    let mut limitations = BTreeSet::new();
    for (requests, results) in calls.values() {
        if !requests.is_empty() && results.is_empty() {
            limitations.insert("missing_selected_tool_result".to_owned());
        } else if requests.len() != 1 || results.len() != 1 {
            limitations.insert("ambiguous_selected_tool_result_binding".to_owned());
        } else if (
            requests[0].reference.turn_index,
            requests[0].reference.part_index,
        ) >= (
            results[0].reference.turn_index,
            results[0].reference.part_index,
        ) {
            limitations.insert("invalid_selected_tool_result_order".to_owned());
        }
    }
    limitations.into_iter().collect()
}

fn retained_part_bytes(
    part: &antiburn_local::analysis::PublishedContentPart,
) -> Result<usize, InputLoadError> {
    let bytes = serde_json::to_vec(&(
        &part.source_key,
        &part.thread_id,
        &part.scope,
        &part.uuid,
        &part.message_id,
        &part.part.text,
        &part.part.tool_name,
        &part.part.tool_call_id,
        &part.part.normalized_fields,
        &part.part.metadata,
    ))
    .map_err(InputLoadError::Serialization)?;
    Ok(bytes.len() + std::mem::size_of_val(part))
}

fn digest(value: &serde_json::Value) -> Result<String, InputLoadError> {
    let bytes = serde_json::to_vec(value).map_err(InputLoadError::Serialization)?;
    Ok(Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

fn unavailable(reason: InputUnavailable) -> InputLoadError {
    InputLoadError::Unavailable(reason)
}

#[cfg(test)]
mod tests;
