//! Adapter from Store publication events to the checks source contract.

use antiburn_checks::domain::{ActivationId, NativeOrderKey};
use antiburn_checks::source::{
    EvidenceValidity, PublicationCursor, PublicationRead, PublishedContext, PublishedEvidenceBatch,
    PublishedEvidenceChange, PublishedEvidenceEvent, PublishedEvidenceReader,
    PublishedEvidenceSource, PublishedResultStatus, PublishedToolResult, SourceError, SourceEvent,
};

use sha2::{Digest, Sha256};

use crate::store::native_tool_results::ToolResultPublicationOutcome;
use crate::store::{SessionKey, Store, native_tool_results::ToolResultPublication};

const FIRST_TERMINAL_PUBLICATION_SQL: &str = "SELECT MIN(p.publication_sequence)
       FROM native_tool_result_publication p
       JOIN native_tool_result_publication current ON current.publication_sequence = ?1
      WHERE p.environment_key = current.environment_key AND p.agent = current.agent
        AND p.session_id = current.session_id AND p.source_format = current.source_format
        AND p.native_identity = current.native_identity
        AND p.session_incarnation IS current.session_incarnation
        AND p.project_cwd IS current.project_cwd AND p.outcome = 'update'
        AND json_extract(p.payload_json, '$.finality') = 1
        AND p.publication_sequence <= current.publication_sequence";

/// Reads committed native-result events from the Store outbox.
///
/// Context comes from the immutable payload, including root-session admission.
pub struct StorePublishedEvidenceReader<'a> {
    store: &'a Store,
    project: Option<(&'a str, &'a str)>,
    live: bool,
    repository_root: Option<&'a str>,
    activation: Option<&'a ActivationId>,
}

impl<'a> StorePublishedEvidenceReader<'a> {
    /// Binds the reader to the application's Store.
    pub fn new(store: &'a Store) -> Self {
        Self {
            store,
            project: None,
            live: false,
            repository_root: None,
            activation: None,
        }
    }

    /// Binds runtime reads to one exact stored project and environment.
    pub fn for_project(store: &'a Store, environment_key: &'a str, project_cwd: &'a str) -> Self {
        Self {
            store,
            project: Some((environment_key, project_cwd)),
            live: false,
            repository_root: None,
            activation: None,
        }
    }

    /// Uses durable publication admission without a consumption-time activity cutoff.
    pub fn for_live_project(
        store: &'a Store,
        environment_key: &'a str,
        project_cwd: &'a str,
    ) -> Self {
        Self {
            live: true,
            ..Self::for_project(store, environment_key, project_cwd)
        }
    }

    /// Sets the resolved repository root without changing the exact source scope.
    pub fn with_repository_root(mut self, root: Option<&'a str>) -> Self {
        self.repository_root = root;
        self
    }

    pub(crate) fn with_activation(mut self, activation: &'a ActivationId) -> Self {
        self.activation = Some(activation);
        self
    }

    fn live_publication(&self, publication: &ToolResultPublication) -> Result<bool, SourceError> {
        self.store
            .lock()
            .query_row(
                "SELECT (COALESCE(?4, ?5 > (SELECT MIN(p.source_generation)
                   FROM native_tool_result_publication p
                   WHERE p.environment_key = ?1 AND p.agent = ?2 AND p.session_id = ?3))
                   AND (?6 IS NULL OR EXISTS(SELECT 1 FROM burn_check_session_attachment t
                     JOIN session s ON s.environment_key = ?1 AND s.agent = ?2 AND s.session_id = ?3
                     WHERE t.check_id = ?6 AND t.activation_scope_key = ?7 AND t.generation = ?8
                       AND t.branch_key = ?9 AND t.incarnation = s.incarnation)))
                   OR EXISTS(SELECT 1 FROM burn_check_observation o
                     JOIN burn_check_activation a USING(check_id, activation_scope_key)
                     WHERE o.check_id = ?6 AND o.activation_scope_key = ?7 AND a.generation = ?8 AND a.enabled = 1
                       AND o.session_key_json = ?9 AND o.native_identity = ?10)",
                rusqlite::params![
                    publication.session.environment_key,
                    publication.session.agent,
                    publication.session.session_id,
                    publication.live_eligible,
                    publication.source_generation,
                    self.activation.map(|id| &id.definition_id),
                    self.activation.map(|id| crate::check_store::StoreCheckStore::activation_scope_key(&id.scope)),
                    self.activation.map(|id| id.generation.0),
                    publication.session.to_identity_json(),
                    publication.native_identity,
                ],
                |row| row.get(0),
            )
            .map_err(|error| SourceError::Reader(error.to_string()))
    }

    fn needs_v2_attachment(
        &self,
        publication: &ToolResultPublication,
    ) -> Result<bool, SourceError> {
        let Some(activation) = self.activation.filter(|_| self.live) else {
            return Ok(false);
        };
        if publication.outcome != ToolResultPublicationOutcome::Update
            || publication.live_eligible != Some(true)
            || !publication.payload.as_ref().is_some_and(|result| {
                result.source_format == antiburn_local::analysis::SourceFormat::OpenCodeSqliteV2
            })
        {
            return Ok(false);
        }
        let source = crate::store::native_tool_results::opencode_v2_status::SOURCE_SQL;
        self.store
            .lock()
            .query_row(
                &format!(
                    "SELECT EXISTS(SELECT 1 FROM session s
               WHERE s.environment_key = ?1 AND s.agent = ?2 AND s.session_id = ?3
                 AND ({source})
                 AND s.cwd = json_extract(?5, '$.session_cwd')
                 AND json_extract(?5, '$.run') = 'Live'
                 AND EXISTS(SELECT 1 FROM opencode_v2_capture v
                   WHERE v.environment_key = s.environment_key AND v.agent = s.agent
                     AND v.session_id = s.session_id AND v.incarnation = s.incarnation
                     AND v.source_label = s.source_label AND v.cwd = s.cwd
                     AND v.capture_epoch <> '' AND v.claim_fence > 0)
                 AND NOT EXISTS(SELECT 1 FROM burn_check_session_attachment t
                   WHERE t.check_id = ?4 AND t.activation_scope_key = ?5 AND t.generation = ?6
                     AND t.branch_key = ?7 AND t.incarnation = s.incarnation))"
                ),
                rusqlite::params![
                    publication.session.environment_key,
                    publication.session.agent,
                    publication.session.session_id,
                    activation.definition_id,
                    crate::check_store::StoreCheckStore::activation_scope_key(&activation.scope),
                    activation.generation.0,
                    publication.session.to_identity_json()
                ],
                |row| row.get(0),
            )
            .map_err(|error| SourceError::Reader(error.to_string()))
    }

    fn first_terminal_publication(
        &self,
        publication: &ToolResultPublication,
    ) -> Result<Option<PublicationCursor>, SourceError> {
        if publication.outcome != ToolResultPublicationOutcome::Update
            || !publication
                .payload
                .as_ref()
                .is_some_and(|result| result.finality)
        {
            return Ok(None);
        }
        let sequence = self
            .store
            .lock()
            .query_row(
                FIRST_TERMINAL_PUBLICATION_SQL,
                [publication.publication_sequence],
                |row| row.get::<_, Option<u64>>(0),
            )
            .map_err(|error| SourceError::Reader(error.to_string()))?;
        Ok(sequence.map(PublicationCursor))
    }

    fn convert_publication(
        &self,
        publication: ToolResultPublication,
    ) -> Result<PublishedEvidenceEvent, SourceError> {
        if self.live
            && publication.outcome == ToolResultPublicationOutcome::Update
            && !self.live_publication(&publication)?
        {
            return Ok(PublishedEvidenceEvent {
                cursor: PublicationCursor(
                    u64::try_from(publication.publication_sequence)
                        .map_err(|error| SourceError::Reader(error.to_string()))?,
                ),
                branch: publication.session.to_identity_json(),
                context: None,
                change: PublishedEvidenceChange::Ignored,
            });
        }
        let terminal = self.first_terminal_publication(&publication)?;
        publication_event(publication, self.repository_root, terminal)
    }

    /// Returns a bounded current window in native order for an explicit startup seed.
    pub(crate) fn recent_results(
        &self,
        session: &SessionKey,
    ) -> Result<Vec<SourceEvent>, SourceError> {
        let sequences = self.store.lock().prepare(
            "SELECT c.publication_sequence FROM native_tool_result_current c
               JOIN native_tool_result_publication p USING(publication_sequence)
               JOIN session s ON s.environment_key = c.environment_key AND s.agent = c.agent
                 AND s.session_id = c.session_id
              WHERE c.environment_key = ?1 AND c.agent = ?2 AND c.session_id = ?3
                AND c.is_removed = 0 AND json_extract(c.payload_json, '$.finality') = 1
                AND (p.session_incarnation IS NULL OR
                  (p.session_incarnation = s.incarnation AND p.project_cwd IS s.cwd))
               ORDER BY COALESCE(json_extract(c.payload_json, '$.context.result_position.FileBytes.record_start'),
                   json_extract(c.payload_json, '$.context.result_position.Jsonl.record_index'),
                   json_extract(c.payload_json, '$.context.result_position.OpenCodeEvent.sequence'),
                   json_extract(c.payload_json, '$.context.result_position.OpenCodeV2Event.sequence')) DESC,
                 COALESCE(json_extract(c.payload_json, '$.context.result_position.FileBytes.item_index'),
                   json_extract(c.payload_json, '$.context.result_position.Jsonl.item_index'),
                   json_extract(c.payload_json, '$.context.result_position.OpenCodeEvent.item_index'),
                   json_extract(c.payload_json, '$.context.result_position.OpenCodeV2Event.item_index')) DESC
              LIMIT 40",
        ).and_then(|mut statement| statement.query_map(
            rusqlite::params![session.environment_key, session.agent, session.session_id],
            |row| row.get::<_, i64>(0),
        )?.collect::<rusqlite::Result<Vec<_>>>())
            .map_err(|error| SourceError::Reader(error.to_string()))?;
        let mut events = Vec::new();
        let mut bytes = 0;
        for sequence in sequences {
            let Some(publication) = self
                .store
                .native_tool_result_publications_after(sequence - 1, 1)
                .map_err(|error| SourceError::Reader(error.to_string()))?
                .pop()
            else {
                continue;
            };
            if publication.publication_sequence != sequence {
                return Err(SourceError::Reader("startup publication changed".into()));
            }
            let size = publication
                .payload
                .as_ref()
                .map(serde_json::to_vec)
                .transpose()
                .map_err(|error| SourceError::Reader(error.to_string()))?
                .map_or(0, |bytes| bytes.len());
            if bytes + size > antiburn_local::analysis::MAX_TOOL_RESULT_BYTES {
                break;
            }
            bytes += size;
            let event = self.convert_publication(publication)?;
            let source = PublishedEvidenceSource::new(OnePublication(event), 1)?;
            events.extend(source.read_batch(PublicationCursor(0))?.events);
        }
        events.reverse();
        Ok(events)
    }
}

struct OnePublication(PublishedEvidenceEvent);

impl PublishedEvidenceReader for OnePublication {
    fn read_batch(&self, _: PublicationRead) -> Result<PublishedEvidenceBatch, SourceError> {
        Ok(PublishedEvidenceBatch {
            events: vec![self.0.clone()],
            next: self.0.cursor,
            has_more: false,
        })
    }
}

impl PublishedEvidenceReader for StorePublishedEvidenceReader<'_> {
    fn read_batch(&self, request: PublicationRead) -> Result<PublishedEvidenceBatch, SourceError> {
        let after = i64::try_from(request.after.0)
            .map_err(|error| SourceError::Reader(error.to_string()))?;
        let global_high_water = self
            .project
            .map(|_| {
                self.store
                    .lock()
                    .query_row(
                        "SELECT COALESCE(MAX(publication_sequence), 0) FROM native_tool_result_publication",
                        [],
                        |row| row.get::<_, i64>(0),
                    )
                    .map_err(|error| SourceError::Reader(error.to_string()))
            })
            .transpose()?;
        let read = |after, limit| match self.project {
            Some((environment, project)) => self
                .store
                .native_tool_result_publications_for_project_after(
                    environment,
                    project,
                    after,
                    limit,
                ),
            None => self
                .store
                .native_tool_result_publications_after(after, limit),
        };
        let mut publications =
            read(after, request.limit).map_err(|error| SourceError::Reader(error.to_string()))?;
        let mut waiting_for_attachment = false;
        for (index, publication) in publications.iter().enumerate() {
            if self.needs_v2_attachment(publication)? {
                publications.truncate(index);
                waiting_for_attachment = true;
                break;
            }
        }
        let next = match publications.last() {
            Some(publication) => PublicationCursor(
                u64::try_from(publication.publication_sequence)
                    .map_err(|error| SourceError::Reader(error.to_string()))?,
            ),
            None if waiting_for_attachment => request.after,
            None => match global_high_water {
                Some(high_water) => PublicationCursor(
                    request.after.0.max(
                        u64::try_from(high_water)
                            .map_err(|error| SourceError::Reader(error.to_string()))?,
                    ),
                ),
                None => request.after,
            },
        };
        let next_sequence =
            i64::try_from(next.0).map_err(|error| SourceError::Reader(error.to_string()))?;
        let has_more = !waiting_for_attachment
            && !read(next_sequence, 1)
                .map_err(|error| SourceError::Reader(error.to_string()))?
                .is_empty();
        let events = publications
            .into_iter()
            .map(|publication| self.convert_publication(publication))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(PublishedEvidenceBatch {
            events,
            next,
            has_more,
        })
    }
}

fn publication_event(
    publication: ToolResultPublication,
    project_root: Option<&str>,
    first_terminal_publication: Option<PublicationCursor>,
) -> Result<PublishedEvidenceEvent, SourceError> {
    let cursor = PublicationCursor(
        u64::try_from(publication.publication_sequence)
            .map_err(|error| SourceError::Reader(error.to_string()))?,
    );
    let branch = publication.session.to_identity_json();
    let context = publication
        .payload
        .as_ref()
        .and_then(|result| result.context.as_ref())
        .map(|context| published_context(context, &publication.session, project_root))
        .transpose()?;
    let change = match publication.outcome {
        ToolResultPublicationOutcome::Update => {
            let result = publication
                .payload
                .ok_or_else(|| SourceError::Reader("update is missing its payload".to_owned()))?;
            let validity = match result.admission() {
                antiburn_local::analysis::ToolResultAdmission::Admitted => EvidenceValidity::Valid,
                antiburn_local::analysis::ToolResultAdmission::Unavailable => {
                    EvidenceValidity::Unknown
                }
                antiburn_local::analysis::ToolResultAdmission::UnsafeIdentity => {
                    EvidenceValidity::UnsafeIdentity
                }
                antiburn_local::analysis::ToolResultAdmission::UnsafeOrder => {
                    EvidenceValidity::UnsafeOrder
                }
                antiburn_local::analysis::ToolResultAdmission::UnsafeOwnership => {
                    EvidenceValidity::UnsafeOwnership
                }
            };
            PublishedEvidenceChange::Upsert(PublishedToolResult {
                first_terminal_publication,
                identity: publication.native_identity.ok_or_else(|| {
                    SourceError::Reader("update is missing its stable identity".to_owned())
                })?,
                native_order: NativeOrderKey(
                    serde_json::to_string(
                        result
                            .context
                            .as_ref()
                            .map_or(&result.position, |context| &context.result_position),
                    )
                    .map_err(|error| SourceError::Reader(error.to_string()))?,
                ),
                revision: result.revision,
                tool_name: result.tool_name,
                category: result.tool_category.map(category_name),
                arguments: result.arguments,
                content: result.content,
                outcome: result
                    .outcome
                    .map(|outcome| {
                        let mut value = serde_json::to_value(outcome)?;
                        if let serde_json::Value::Object(fields) = &mut value {
                            fields.retain(|_, value| !value.is_null());
                        }
                        Ok::<_, serde_json::Error>(value)
                    })
                    .transpose()
                    .map_err(|error| SourceError::Reader(error.to_string()))?,
                provider_executed: result.provider_executed,
                status: match result.status {
                    antiburn_local::analysis::ToolResultStatus::Success => {
                        PublishedResultStatus::Completed
                    }
                    antiburn_local::analysis::ToolResultStatus::Error => {
                        PublishedResultStatus::Error
                    }
                    antiburn_local::analysis::ToolResultStatus::Interrupted => {
                        PublishedResultStatus::Interrupted
                    }
                    antiburn_local::analysis::ToolResultStatus::Unknown => {
                        PublishedResultStatus::Unknown
                    }
                },
                finality: result.finality,
                payload_complete: result.finality
                    && result.completeness
                        == antiburn_local::analysis::ToolResultCompleteness::Complete,
                validity,
            })
        }
        ToolResultPublicationOutcome::Removal => PublishedEvidenceChange::Removal {
            identity: publication
                .native_identity
                .ok_or_else(|| SourceError::Reader("removal is missing its identity".to_owned()))?,
            revision: u64::try_from(publication.native_revision.ok_or_else(|| {
                SourceError::Reader("removal is missing its revision".to_owned())
            })?)
            .map_err(|error| SourceError::Reader(error.to_string()))?,
        },
        ToolResultPublicationOutcome::CoverageGap => {
            let reason = publication
                .gap_reason
                .unwrap_or_else(|| "published source membership is incomplete".to_owned());
            if publication.source_format.as_deref() == Some("\"open_code_v2_sqlite\"")
                && reason != "v2-history-incomplete"
            {
                PublishedEvidenceChange::SegmentReset { reason }
            } else {
                PublishedEvidenceChange::Gap { reason }
            }
        }
        ToolResultPublicationOutcome::CoverageRestored => PublishedEvidenceChange::CoverageRestored,
    };
    Ok(PublishedEvidenceEvent {
        cursor,
        branch,
        context,
        change,
    })
}

fn published_context(
    context: &antiburn_local::analysis::ToolResultContext,
    session: &SessionKey,
    project_root: Option<&str>,
) -> Result<PublishedContext, SourceError> {
    use antiburn_local::analysis::{UserMessageChange, UserMessageCompleteness};

    let encoded =
        serde_json::to_vec(context).map_err(|error| SourceError::Reader(error.to_string()))?;
    let complete = context.complete_through_event
        && context.main_session
        && context.user_messages.iter().all(|message| {
            message.is_bounded()
                && message.session_id == session.session_id
                && message.change == UserMessageChange::Present
                && message.completeness == UserMessageCompleteness::Complete
        });
    Ok(PublishedContext {
        requests: context.user_messages.iter().map(|message| {
            (message.message_id.clone(), serde_json::json!({
                "source": session.agent,
                "kind": "user_message",
                "text": message.parts.iter().map(|part| part.text.as_str()).collect::<Vec<_>>().join("\n"),
            }))
        }).collect(),
        session_id: session.session_id.clone(),
        agent_name: session.agent.clone(),
        project_root: project_root.map(str::to_owned),
        version: Sha256::digest(encoded).iter().map(|byte| format!("{byte:02x}")).collect(),
        main_session: context.main_session,
        complete_through_event: complete,
    })
}

fn category_name(category: antiburn_local::analysis::ToolCategory) -> String {
    use antiburn_local::analysis::ToolCategory;
    match category {
        ToolCategory::Bash | ToolCategory::Test => "shell",
        ToolCategory::Edit => "edit",
        ToolCategory::Read => "read",
        ToolCategory::Search => "search",
        ToolCategory::Other => "other",
    }
    .to_owned()
}

#[cfg(test)]
mod tests;
