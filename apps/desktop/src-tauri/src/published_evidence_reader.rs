//! Adapter from Store publication events to the checks source contract.

use antiburn_checks::domain::{ActivationId, NativeOrderKey};
use antiburn_checks::source::{
    EvidenceValidity, PublicationCursor, PublicationRead, PublishedContext, PublishedEvidenceBatch,
    PublishedEvidenceChange, PublishedEvidenceEvent, PublishedEvidenceReader,
    PublishedEvidenceSource, PublishedResultStatus, PublishedToolResult, SourceError, SourceEvent,
};

use crate::store::burn_checks::{
    trace_failure::{FailureCause, FailureLog},
    trace_refs,
};
use sha2::{Digest, Sha256};

use crate::store::native_tool_results::ToolResultPublicationOutcome;
use crate::store::native_tool_results::publications::PublicationGapKind;
use crate::store::{SessionKey, Store, native_tool_results::ToolResultPublication};

mod generic;

#[derive(Clone, Copy)]
enum ReaderStage {
    Read,
    Decode,
}

impl ReaderStage {
    fn as_str(self) -> &'static str {
        match self {
            Self::Read => "publication_read",
            Self::Decode => "publication_decode",
        }
    }
}

fn reader_failures() -> &'static FailureLog {
    static PREVIOUS: std::sync::OnceLock<FailureLog> = std::sync::OnceLock::new();
    PREVIOUS.get_or_init(Default::default)
}

fn trace_reader_error<E: std::fmt::Display + 'static>(
    reader: &StorePublishedEvidenceReader<'_>,
    stage: ReaderStage,
    error: E,
) -> SourceError {
    let cause = FailureCause::classify_value(&error);
    let reference = reader.trace_reference();
    let stage = stage.as_str();
    if reader_failures().changed(reference.clone(), stage, Some(cause)) {
        ::tracing::error!(event = "burn_check_publication_read_failed", stage, source_ref = reference,
            activation_ref = reader.activation.map(trace_refs::activation_ref),
            reason = cause.category, sqlite_code = cause.sqlite_code, io_kind = ?cause.io_kind);
    }
    SourceError::Reader(error.to_string())
}

const FIRST_TERMINAL_PUBLICATION_SQL: &str = "SELECT MIN(p.publication_sequence)
       FROM native_tool_result_publication p
       JOIN native_tool_result_publication current ON current.publication_sequence = ?1
      WHERE p.environment_key = current.environment_key AND p.agent = current.agent
        AND p.session_id = current.session_id AND p.source_format = current.source_format
        AND p.native_identity = current.native_identity
        AND p.event_kind = 'tool_result' AND current.event_kind = 'tool_result'
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
    fn trace_reference(&self) -> String {
        self.activation
            .map(trace_refs::activation_ref)
            .unwrap_or_else(|| match self.project {
                Some((environment, project)) => {
                    trace_refs::scope_ref(&antiburn_checks::domain::ActivationScope {
                        environment: environment.into(),
                        session_cwd: project.into(),
                        run: antiburn_checks::domain::RunScope::Live,
                    })
                }
                None => trace_refs::report_ref("publication_reader"),
            })
    }

    fn reader_error<E: std::fmt::Display + 'static>(&self, error: E) -> SourceError {
        trace_reader_error(self, ReaderStage::Read, error)
    }

    fn decode_error<E: std::fmt::Display + 'static>(&self, error: E) -> SourceError {
        trace_reader_error(self, ReaderStage::Decode, error)
    }

    fn trace_read_success(&self) {
        for stage in [ReaderStage::Read, ReaderStage::Decode] {
            reader_failures().changed(self.trace_reference(), stage.as_str(), None);
        }
    }

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
        let root_session = crate::store::selected_source::ROOT_SESSION_SQL;
        self.store
            .lock()
            .query_row(
                &format!("SELECT (COALESCE(?4, ?5 > (SELECT MIN(p.source_generation)
                   FROM native_tool_result_publication p
                   WHERE p.environment_key = ?1 AND p.agent = ?2 AND p.session_id = ?3))
                   AND (?6 IS NULL OR EXISTS(SELECT 1 FROM burn_check_session_attachment t
                     JOIN session s ON s.environment_key = ?1 AND s.agent = ?2 AND s.session_id = ?3
                     WHERE t.check_id = ?6 AND t.activation_scope_key = ?7 AND t.generation = ?8
                       AND t.branch_key = ?9 AND t.incarnation = s.incarnation
                       AND s.wsl_distro IS NULL AND ({root_session}))))
                   OR EXISTS(SELECT 1 FROM burn_check_observation o
                     JOIN burn_check_activation a USING(check_id, activation_scope_key)
                     WHERE o.check_id = ?6 AND o.activation_scope_key = ?7 AND a.generation = ?8 AND a.enabled = 1
                       AND o.session_key_json = ?9 AND o.native_identity = ?10)"),
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
            .map_err(|error| self.reader_error(error))
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
            .map_err(|error| self.reader_error(error))?;
        Ok(sequence.map(PublicationCursor))
    }

    fn convert_publication(
        &self,
        publication: ToolResultPublication,
    ) -> Result<PublishedEvidenceEvent, SourceError> {
        if !self.request_origin_allowed(publication.publication_sequence)? {
            return Ok(PublishedEvidenceEvent {
                cursor: PublicationCursor(
                    u64::try_from(publication.publication_sequence)
                        .map_err(|error| self.decode_error(error))?,
                ),
                branch: publication.session.to_identity_json(),
                context: None,
                source_evidence_version: None,
                change: PublishedEvidenceChange::Ignored,
            });
        }
        if publication.outcome == ToolResultPublicationOutcome::Update
            && let Some(reason) = crate::store::native_tool_results::admission::event_reason_in(
                &self.store.lock(),
                &publication.session,
                publication.publication_sequence,
            )
            .map_err(|error| self.reader_error(error))?
        {
            return Ok(PublishedEvidenceEvent {
                cursor: PublicationCursor(
                    u64::try_from(publication.publication_sequence)
                        .map_err(|error| self.decode_error(error))?,
                ),
                branch: publication.session.to_identity_json(),
                context: None,
                source_evidence_version: None,
                change: PublishedEvidenceChange::UnsafeUpdate { reason },
            });
        }
        if self.live
            && publication.outcome == ToolResultPublicationOutcome::Update
            && !self.live_publication(&publication)?
        {
            return Ok(PublishedEvidenceEvent {
                source_evidence_version: None,
                cursor: PublicationCursor(
                    u64::try_from(publication.publication_sequence)
                        .map_err(|error| self.decode_error(error))?,
                ),
                branch: publication.session.to_identity_json(),
                context: None,
                change: PublishedEvidenceChange::Ignored,
            });
        }
        let terminal = self.first_terminal_publication(&publication)?;
        publication_event(self, publication, self.repository_root, terminal)
    }

    /// Returns a current result window before the first held live event.
    pub(crate) fn recent_results_before(
        &self,
        session: &SessionKey,
        before_publication: Option<i64>,
    ) -> Result<Vec<SourceEvent>, SourceError> {
        let v2_format =
            serde_json::to_string(&antiburn_local::analysis::SourceFormat::OpenCodeSqliteV2)
                .map_err(|error| self.decode_error(error))?;
        let live = self.activation.map_or(self.live, |activation| {
            activation.scope.run == antiburn_checks::domain::RunScope::Live
        });
        let origin = if live {
            crate::store::native_tool_results::request_origin::publication_allowed_sql("p")
        } else {
            "1".into()
        };
        let sequences = self.store.lock().prepare(
            &format!("SELECT c.publication_sequence FROM native_tool_result_current c
               JOIN native_tool_result_publication p USING(publication_sequence)
               JOIN session s ON s.environment_key = c.environment_key AND s.agent = c.agent
                 AND s.session_id = c.session_id
              WHERE c.environment_key = ?1 AND c.agent = ?2 AND c.session_id = ?3
                 AND (s.source_format IS NULL OR s.source_format <> ?5)
                 AND c.is_removed = 0 AND json_extract(c.payload_json, '$.finality') = 1
                  AND c.event_kind = 'tool_result'
                  AND {origin}
                 AND (?4 IS NULL OR c.publication_sequence < ?4)
                AND (p.session_incarnation IS NULL OR
                  (p.session_incarnation = s.incarnation AND p.project_cwd IS s.cwd))
               ORDER BY COALESCE(json_extract(c.payload_json, '$.context.result_position.FileBytes.record_start'),
                   json_extract(c.payload_json, '$.context.result_position.Jsonl.record_index'),
                    json_extract(c.payload_json, '$.context.result_position.OpenCodeEvent.sequence')) DESC,
                 COALESCE(json_extract(c.payload_json, '$.context.result_position.FileBytes.item_index'),
                   json_extract(c.payload_json, '$.context.result_position.Jsonl.item_index'),
                    json_extract(c.payload_json, '$.context.result_position.OpenCodeEvent.item_index')) DESC
               LIMIT 40"),
        ).and_then(|mut statement| statement.query_map(
            rusqlite::params![session.environment_key, session.agent, session.session_id,
                before_publication, v2_format],
            |row| row.get::<_, i64>(0),
        )?.collect::<rusqlite::Result<Vec<_>>>())
            .map_err(|error| self.reader_error(error))?;
        let mut events = Vec::new();
        let mut bytes = 0;
        for sequence in sequences {
            let Some(publication) = self
                .store
                .native_tool_result_publications_after(sequence - 1, 1)
                .map_err(|error| self.reader_error(error))?
                .pop()
            else {
                continue;
            };
            if publication.publication_sequence != sequence {
                return Err(self.reader_error("startup publication changed"));
            }
            let size = publication
                .payload
                .as_ref()
                .map(serde_json::to_vec)
                .transpose()
                .map_err(|error| self.decode_error(error))?
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
        self.trace_read_success();
        Ok(events)
    }

    fn request_origin_allowed(&self, sequence: i64) -> Result<bool, SourceError> {
        if !self.live
            || self.activation.is_some_and(|activation| {
                activation.scope.run != antiburn_checks::domain::RunScope::Live
            })
        {
            return Ok(true);
        }
        crate::store::native_tool_results::request_origin::publication_allowed_in(
            &self.store.lock(),
            sequence,
        )
        .map_err(|error| self.reader_error(error))
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
        let started = std::time::Instant::now();
        let after = i64::try_from(request.after.0).map_err(|error| self.decode_error(error))?;
        // Read the bound first so an empty page cannot skip a later publication.
        let global_high_water = self
            .project
            .map(|_| {
                self.store.lock().query_row(
                    "SELECT COALESCE(MAX(publication_sequence), 0) FROM native_tool_result_publication",
                    [],
                    |row| row.get::<_, u64>(0),
                )
                .map_err(|error| self.reader_error(error))
            })
            .transpose()?;
        let read = |after, limit| match self.project {
            Some((environment, project)) => {
                self.store.native_evidence_publications_for_project_after(
                    environment,
                    project,
                    self.activation,
                    after,
                    limit,
                )
            }
            None => self.store.native_evidence_publications_after(after, limit),
        };
        let mut publications =
            read(after, request.limit).map_err(|error| self.reader_error(error))?;
        let mut waiting_for_attachment = false;
        for (index, publication) in publications.iter().enumerate() {
            if self.generic_needs_attachment(publication)? {
                publications.truncate(index);
                waiting_for_attachment = true;
                break;
            }
        }
        let next = match publications.last() {
            Some(publication) => PublicationCursor(
                u64::try_from(publication.publication_sequence)
                    .map_err(|error| self.decode_error(error))?,
            ),
            None if !waiting_for_attachment => {
                PublicationCursor(request.after.0.max(global_high_water.unwrap_or(0)))
            }
            None => request.after,
        };
        let next_sequence = i64::try_from(next.0).map_err(|error| self.decode_error(error))?;
        let has_more = !waiting_for_attachment
            && !read(next_sequence, 1)
                .map_err(|error| self.reader_error(error))?
                .is_empty();
        let events = publications
            .into_iter()
            .map(|publication| self.convert_evidence(publication))
            .collect::<Result<Vec<_>, _>>()?;
        self.trace_read_success();
        if !events.is_empty() {
            ::tracing::debug!(
                event = "burn_check_publications_read",
                activation_ref = self
                    .activation
                    .map(crate::store::burn_checks::trace_refs::activation_ref),
                generation = self.activation.map(|id| id.generation.0),
                events = events.len(),
                publication_cursor = next.0,
                live = self.live,
                waiting_for_attachment,
                has_more,
                elapsed_ms = started.elapsed().as_millis() as u64,
            );
        }
        Ok(PublishedEvidenceBatch {
            events,
            next,
            has_more,
        })
    }
}

fn publication_event(
    reader: &StorePublishedEvidenceReader<'_>,
    publication: ToolResultPublication,
    project_root: Option<&str>,
    first_terminal_publication: Option<PublicationCursor>,
) -> Result<PublishedEvidenceEvent, SourceError> {
    let cursor = PublicationCursor(
        u64::try_from(publication.publication_sequence)
            .map_err(|error| reader.decode_error(error))?,
    );
    let branch = publication.session.to_identity_json();
    let context = publication
        .payload
        .as_ref()
        .and_then(|result| result.context.as_ref())
        .map(|context| {
            published_context(
                reader,
                context,
                &publication.session,
                project_root,
                publication.session_selection.clone(),
            )
        })
        .transpose()?;
    let change = match publication.outcome {
        ToolResultPublicationOutcome::Update => {
            let result = publication
                .payload
                .ok_or_else(|| reader.decode_error("update is missing its payload"))?;
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
            let metadata =
                published_tool_metadata(result.facts.as_ref(), result.source_format, project_root);
            let legacy_category = if result.facts.is_none() {
                result.tool_category.map(category_name)
            } else {
                None
            };
            PublishedEvidenceChange::Upsert(PublishedToolResult {
                metadata,
                first_terminal_publication,
                identity: publication
                    .native_identity
                    .ok_or_else(|| reader.decode_error("update is missing its stable identity"))?,
                native_order: NativeOrderKey(
                    serde_json::to_string(
                        result
                            .context
                            .as_ref()
                            .map_or(&result.position, |context| &context.result_position),
                    )
                    .map_err(|error| reader.decode_error(error))?,
                ),
                revision: result.revision,
                tool_name: result.tool_name,
                category: legacy_category,
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
                    .map_err(|error| reader.decode_error(error))?,
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
                .ok_or_else(|| reader.decode_error("removal is missing its identity"))?,
            revision: u64::try_from(
                publication
                    .native_revision
                    .ok_or_else(|| reader.decode_error("removal is missing its revision"))?,
            )
            .map_err(|error| reader.decode_error(error))?,
        },
        ToolResultPublicationOutcome::CoverageGap => {
            let reason = publication
                .gap_reason
                .unwrap_or_else(|| "published source membership is incomplete".to_owned());
            if crate::store::native_tool_results::admission::is_unsafe(&reason) {
                PublishedEvidenceChange::UnsafeUpdate { reason }
            } else {
                match publication.gap_kind {
                    PublicationGapKind::Invalidated => PublishedEvidenceChange::Gap { reason },
                    PublicationGapKind::Partial => PublishedEvidenceChange::PartialGap { reason },
                    PublicationGapKind::SegmentReset => {
                        PublishedEvidenceChange::SegmentReset { reason }
                    }
                }
            }
        }
        ToolResultPublicationOutcome::CoverageRestored => PublishedEvidenceChange::CoverageRestored,
    };
    Ok(PublishedEvidenceEvent {
        source_evidence_version: None,
        cursor,
        branch,
        context,
        change,
    })
}

fn published_context(
    reader: &StorePublishedEvidenceReader<'_>,
    context: &antiburn_local::analysis::ToolResultContext,
    session: &SessionKey,
    project_root: Option<&str>,
    session_selection: Option<antiburn_checks::context::ObservedAttribution>,
) -> Result<PublishedContext, SourceError> {
    use antiburn_local::analysis::{UserMessageChange, UserMessageCompleteness};

    let encoded = serde_json::to_vec(context).map_err(|error| reader.decode_error(error))?;
    let complete = context.complete_through_event
        && context.main_session
        && context.user_messages.iter().all(|message| {
            message.is_bounded()
                && message.session_id == session.session_id
                && message.change == UserMessageChange::Present
                && message.completeness == UserMessageCompleteness::Complete
        });
    Ok(PublishedContext {
        inventories: Default::default(),
        session_selection,
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

fn published_tool_metadata(
    facts: Option<&antiburn_local::analysis::NativeToolFacts>,
    source_format: antiburn_local::analysis::SourceFormat,
    project_root: Option<&str>,
) -> Box<antiburn_checks::source::PublishedToolMetadata> {
    use antiburn_checks::compiler::ToolCategory;
    use antiburn_checks::context::ObservedAttribution;
    use antiburn_checks::source::PublishedToolMetadata;
    use antiburn_local::analysis::CheckToolCategory;
    use antiburn_local::analysis::SourceFormat;

    let Some(facts) = facts else {
        return Box::default();
    };
    let paths = facts
        .operation_paths
        .as_ref()
        .and_then(|paths| project_root.and_then(|root| paths.repo_relative_paths(root)));
    let paths_complete = paths.is_some();
    let model = facts.requester.model_key();
    Box::new(PublishedToolMetadata {
        fast_mode_evidence: facts.requester.fast_mode.and_then(|_| {
            match source_format {
                SourceFormat::ClaudeJsonl => Some("reported-speed"),
                SourceFormat::CodexRolloutJsonl => Some("requested-tier-preference"),
                SourceFormat::OpenCodeSqliteV2 => Some("selected-catalog-model"),
                SourceFormat::PiV3Jsonl => Some("model-id-suffix-hint"),
                _ => None,
            }
            .map(str::to_owned)
        }),
        model_limit: model
            .is_none()
            .then(|| "requester-model-unavailable".into()),
        fast_mode_limit: facts
            .requester
            .fast_mode
            .is_none()
            .then(|| "recorded-requester-mode-unavailable".into()),
        attribution: ObservedAttribution {
            model,
            fast_mode: facts.requester.fast_mode,
        },
        check_category: facts.category.map(|category| match category {
            CheckToolCategory::Shell => ToolCategory::Shell,
            CheckToolCategory::Read => ToolCategory::Read,
            CheckToolCategory::Search => ToolCategory::Search,
            CheckToolCategory::Edit => ToolCategory::Edit,
            CheckToolCategory::Write => ToolCategory::Write,
            CheckToolCategory::Delegate => ToolCategory::Delegate,
            CheckToolCategory::Skill => ToolCategory::Skill,
            CheckToolCategory::Mcp => ToolCategory::Mcp,
            CheckToolCategory::Other => ToolCategory::Other,
        }),
        category_limit: facts
            .category
            .is_none()
            .then(|| "native-tool-category-unavailable".into()),
        paths,
        paths_complete,
        paths_limits: if paths_complete {
            Vec::new()
        } else {
            vec![
                if project_root.is_none() {
                    "repository-root-unavailable"
                } else if facts.operation_paths.is_none() {
                    "operation-paths-unavailable"
                } else {
                    "operation-path-binding-unavailable"
                }
                .into(),
            ]
        },
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
