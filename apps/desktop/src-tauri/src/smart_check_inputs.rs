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

mod episodes;
mod inventory;
pub use episodes::{EpisodeCompletion, InvestigationSpan};
pub use inventory::{InventoryRevisionChange, InventoryRevisionObserver, SkillInputs};

const MAX_ACTIVITY_BYTES: usize = 16 * 1024 * 1024;

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
        let request = self
            .session_scope_request(key, publication_fence, source_generation)
            .map_err(InputLoadError::Scope)?;
        let boundary = request.boundary().clone();
        let source_format = request.source_format();
        let scope = Arc::new(
            self.load_session_scope(key, request)
                .map_err(InputLoadError::Scope)?,
        );
        let published = self.collect_smart_check_activity(
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
        let content = select_session_content(
            &prepare_session_content(&identity, source_format, published, Vec::new()),
            detector.selection(),
        );
        if !content.complete {
            return Err(unavailable(InputUnavailable::IncompleteEvidence));
        }
        let investigation_spans = if detector == DetectorInput::OverExploring {
            episodes::investigation_spans(&content, &scope)?
        } else {
            Vec::new()
        };
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
        Ok(SmartCheckInputSnapshot {
            key: key.clone(),
            detector,
            scope,
            boundary,
            content,
            revision,
            investigation_spans,
        })
    }

    fn collect_smart_check_activity(
        &self,
        key: &SessionKey,
        publication_fence: i64,
        source_generation: i64,
        boundary: &SessionScopeBoundary,
        detector: DetectorInput,
    ) -> Result<PublishedContent, InputLoadError> {
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
        loop {
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
            if page.boundary.is_none() {
                return Err(unavailable(InputUnavailable::BoundaryMissing));
            }
            let coverage = page.page.content.coverage;
            if coverage.oversized_parts != 0
                || coverage.stored_truncated_parts != 0
                || coverage.context_capped
                || ((coverage.bytes_capped || coverage.parts_capped || coverage.more_parts)
                    && page.page.next_cursor.is_none())
            {
                return Err(unavailable(InputUnavailable::IncompleteEvidence));
            }
            for part in page.page.content.parts {
                if part.part.normalized_fields.as_ref().is_some_and(|fields| {
                    fields.malformed
                        || (part.part.kind.as_str() == "tool_input" && fields.values.is_empty())
                }) {
                    return Err(unavailable(InputUnavailable::IncompleteEvidence));
                }
                let position = (part.turn_index, part.part_index);
                if previous.is_some_and(|last| last >= position)
                    || part.source_key != boundary.source_key
                    || part.thread_id != boundary.thread_id
                    || part.scope != "main"
                    || part.context_only
                    || !part.stable_event_identity
                {
                    return Err(unavailable(InputUnavailable::InvalidEventOrder));
                }
                previous = Some(position);
                let part_bytes = retained_part_bytes(&part)?;
                bytes = bytes
                    .checked_add(part_bytes)
                    .ok_or_else(|| unavailable(InputUnavailable::AssemblyLimitReached))?;
                if bytes > MAX_ACTIVITY_BYTES || collected.parts.len() == detector.max_events() {
                    return Err(unavailable(InputUnavailable::AssemblyLimitReached));
                }
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
        Ok(collected)
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
