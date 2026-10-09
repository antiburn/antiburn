//! Full scope reads use the published store, including remote and WSL caches.
//! The environment key keeps native, remote, and WSL sessions separate. This
//! loader never reads a current companion file or scans the filesystem.

use std::collections::BTreeMap;

use antiburn_local::analysis::session_scope::{
    SCOPE_SELECTION, ScopeMissingReason, SessionScopeBoundary, SessionScopeBuilder,
    SessionScopeError, SessionScopeSnapshot,
};
use antiburn_local::analysis::{SelectedContentQueryError, SelectedContentRequest, SourceFormat};

use crate::store::{SessionKey, Store};

#[derive(Debug)]
pub enum ScopeLoadError {
    Scope(SessionScopeError),
    Query(SelectedContentQueryError),
}

pub struct ScopeLoadRequest {
    source_format: SourceFormat,
    boundary: SessionScopeBoundary,
    publication_fence: i64,
    source_generation: i64,
    /// True only when the source retains complete recorded user text with no known loss.
    source_complete: bool,
    untrusted_user_parts: std::collections::BTreeSet<(u64, u32)>,
    key: SessionKey,
}

impl ScopeLoadRequest {
    pub fn boundary(&self) -> &SessionScopeBoundary {
        &self.boundary
    }

    pub fn source_format(&self) -> SourceFormat {
        self.source_format
    }
}

impl Store {
    /// Load from source start, including messages before check enablement.
    /// Obtain the request from `session_scope_request` at the assessment revision.
    pub fn load_session_scope(
        &self,
        key: &SessionKey,
        request: ScopeLoadRequest,
    ) -> Result<SessionScopeSnapshot, ScopeLoadError> {
        if request.key != *key {
            return Err(ScopeLoadError::Scope(SessionScopeError::Missing(
                ScopeMissingReason::InvalidEvidence,
            )));
        }
        let ScopeLoadRequest {
            source_format,
            boundary,
            publication_fence,
            source_generation,
            source_complete,
            untrusted_user_parts,
            key: _,
        } = request;
        let content_scope = boundary.content_scope().map_err(ScopeLoadError::Scope)?;
        let mut builder = SessionScopeBuilder::new(
            source_format,
            boundary,
            publication_fence,
            source_generation,
            source_complete,
        )
        .map_err(ScopeLoadError::Scope)?;
        let positions = BTreeMap::new();
        let mut cursor = None;
        loop {
            let mut page = self
                .published_turn_content_keyset_scoped(
                    key,
                    SelectedContentRequest {
                        source_generation,
                        after_ms: None,
                        source_positions: &positions,
                        selection: SCOPE_SELECTION,
                        cursor: cursor.as_ref(),
                    },
                    &content_scope,
                )
                .map_err(ScopeLoadError::Query)?
                .ok_or(ScopeLoadError::Scope(SessionScopeError::Missing(
                    ScopeMissingReason::PublicationChanged,
                )))?;
            if page.page.content.publication_fence != publication_fence {
                return Err(ScopeLoadError::Scope(SessionScopeError::Missing(
                    ScopeMissingReason::PublicationChanged,
                )));
            }
            let proof =
                page.boundary
                    .as_ref()
                    .ok_or(ScopeLoadError::Scope(SessionScopeError::Missing(
                        ScopeMissingReason::BoundaryMissing,
                    )))?;
            builder
                .bind_boundary(proof)
                .map_err(ScopeLoadError::Scope)?;
            page.page
                .content
                .parts
                .retain(|part| !untrusted_user_parts.contains(&(part.turn_index, part.part_index)));
            builder
                .push_page(page.page.content, page.page.next_cursor.is_some())
                .map_err(ScopeLoadError::Scope)?;
            cursor = page.page.next_cursor;
            if cursor.is_none() {
                break;
            }
        }
        builder.finish().map_err(ScopeLoadError::Scope)
    }
}

#[cfg(test)]
mod tests;

mod factory;
