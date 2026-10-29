use std::collections::BTreeMap;

use antiburn_checks::domain::{BranchId, EvidenceDependency};
use antiburn_checks::source::SourceEvent;

use super::*;
use crate::store::SessionKey;
use crate::store::burn_checks::observations::{BurnCheckPublicationPage, RuntimeCheckpoint};
use crate::store::burn_checks::{BurnCheckCandidateRecord, BurnCheckObservationRecord};

struct PublicationMetadata {
    session: SessionKey,
    source_format: String,
    native_identity: String,
    revision: i64,
}

impl StoreCheckStore {
    pub(super) fn progress(&self, activation: &EffectiveActivation) -> Result<SourceProgress> {
        self.validate_activation(activation)?;
        let saved: String = self.store.lock().query_row(
            "SELECT checkpoint_json FROM burn_check_activation
            WHERE check_id = ?1 AND activation_scope_key = ?2 AND generation = ?3 AND enabled = 1",
            params![
                activation.id.definition_id,
                self.activation_scope_key,
                integer(activation.id.generation.0)?
            ],
            |row| row.get(0),
        )?;
        SourceProgress::decode(&saved).map_err(|error| anyhow::anyhow!(error.message))
    }

    fn publication_metadata(
        &self,
        cursor: PublicationCursor,
        branch: &BranchId,
        identity: &str,
        revision: u64,
    ) -> Result<PublicationMetadata> {
        if matches!(self.scope.run, RunScope::Replay(_)) {
            return Ok(PublicationMetadata {
                session: SessionKey::new(&self.scope.environment, "replay", &branch.0),
                source_format: "normalized_replay".into(),
                native_identity: identity.into(),
                revision: integer(revision)?,
            });
        }
        self.store.lock().query_row("SELECT environment_key, agent, session_id, source_format, COALESCE(native_identity, ?2), COALESCE(native_revision, ?3)
            FROM native_tool_result_publication WHERE publication_sequence = ?1",
            params![integer(cursor.0)?, identity, integer(revision)?], |row| Ok(PublicationMetadata {
                session: SessionKey::new(row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?),
                source_format: row.get(3)?, native_identity: row.get(4)?, revision: row.get(5)?,
            })).context("source publication is unavailable")
    }

    pub(super) fn persist_page(
        &self,
        activation: &EffectiveActivation,
        expected: SourceProgressHead,
        page: &SourceBatch,
        committed: &[CommittedSourceObservation],
        next: &SourceProgress,
        candidates: &[FrozenCandidate],
    ) -> Result<()> {
        let started = Instant::now();
        let saved = self.progress(activation)?;
        ensure!(
            saved.head() == expected,
            "source progress compare-and-swap failed"
        );
        ensure!(
            next.publication_cursor == page.next,
            "checkpoint does not acknowledge the page"
        );
        ensure!(
            next.observation_position.0
                == expected
                    .observation_position
                    .0
                    .checked_add(committed.len() as u64)
                    .context("observation order exhausted")?,
            "checkpoint observation order does not match the page"
        );
        let mut observations = Vec::new();
        let mut sequences = Vec::new();
        let mut invalidated_branches = Vec::new();
        invalidated_branches.extend(
            committed
                .iter()
                .filter(|item| item.window_invalidated)
                .map(|item| item.observation.branch.0.clone()),
        );
        let mut reset_branches = Vec::new();
        let mut triggers = BTreeMap::new();
        for event in &page.events {
            let cursor = match event {
                SourceEvent::Ignored { cursor } => *cursor,
                SourceEvent::Observation {
                    cursor,
                    identity,
                    revision,
                    observation: draft,
                    ..
                } => {
                    let item = committed
                        .iter()
                        .find(|item| item.cursor == *cursor)
                        .context("source observation was not committed")?;
                    ensure!(
                        item.identity == *identity
                            && item.revision == *revision
                            && item.observation
                                == (**draft).clone().commit(item.observation.position),
                        "committed observation does not match source event"
                    );
                    let metadata = self.publication_metadata(
                        *cursor,
                        &item.observation.branch,
                        identity,
                        *revision,
                    )?;
                    ensure!(
                        metadata.revision == integer(*revision)?,
                        "source revision changed"
                    );
                    triggers.insert(
                        item.observation.position,
                        (metadata.session.clone(), *cursor),
                    );
                    observations.push(BurnCheckObservationRecord {
                        publication_sequence: integer(cursor.0)?,
                        session: metadata.session,
                        observation_id: observation_key(&item.observation.branch, identity),
                        source_format: metadata.source_format,
                        native_identity: metadata.native_identity,
                        native_revision: metadata.revision,
                        evidence_version: item.observation.version.0.clone(),
                        removed: false,
                        payload: serde_json::to_value(draft)?,
                    });
                    *cursor
                }
                SourceEvent::Removal {
                    cursor,
                    identity,
                    revision,
                    branch,
                } => {
                    let metadata =
                        self.publication_metadata(*cursor, branch, identity, *revision)?;
                    observations.push(BurnCheckObservationRecord {
                        publication_sequence: integer(cursor.0)?,
                        session: metadata.session,
                        observation_id: observation_key(branch, identity),
                        source_format: metadata.source_format,
                        native_identity: metadata.native_identity,
                        native_revision: metadata.revision,
                        evidence_version: format!("removed:{revision}"),
                        removed: true,
                        payload: serde_json::json!({"removed": true}),
                    });
                    *cursor
                }
                SourceEvent::SegmentReset { cursor, branch, .. } => {
                    reset_branches.push(branch.0.clone());
                    *cursor
                }
                SourceEvent::UnsafeUpdate { cursor, branch, .. } => {
                    invalidated_branches.push(branch.0.clone());
                    *cursor
                }
                SourceEvent::Gap { cursor, .. } | SourceEvent::CoverageRestored { cursor, .. } => {
                    *cursor
                }
            };
            sequences.push(integer(cursor.0)?);
        }
        let records = candidates
            .iter()
            .map(|candidate| {
                ensure!(
                    candidate.activation == activation.id,
                    "candidate belongs to another activation"
                );
                let (session, cursor) = triggers
                    .get(&candidate.trigger_position)
                    .context("candidate trigger is outside the source page")?;
                let mut dependencies = serde_json::Map::new();
                for dependency in &candidate.dependencies {
                    if let EvidenceDependency::Message { id, version } = dependency {
                        dependencies.insert(
                            observation_key(&candidate.branch, id),
                            serde_json::Value::String(version.0.clone()),
                        );
                    }
                    if let EvidenceDependency::SourceEvidence { id, version } = dependency {
                        let trigger = committed.iter().find(|item| item.observation.position == candidate.trigger_position)
                            .context("candidate trigger has no committed observation")?;
                        if id == &trigger.identity && version == &trigger.observation.version {
                            dependencies.insert(observation_key(&candidate.branch, id), serde_json::Value::String(version.0.clone()));
                        } else {
                            let binding_json: Option<String> = self.store.lock().query_row(
                                "SELECT dependency_binding_json FROM native_tool_result_publication WHERE publication_sequence = ?1",
                                [integer(cursor.0)?], |r| r.get(0)).optional()?.flatten();
                            let bindings: Vec<crate::store::native_tool_results::evidence::NativeCurrentBinding> = binding_json
                                .map(|json| serde_json::from_str(&json)).transpose()?.unwrap_or_default();
                            let binding = bindings.into_iter().find(|binding| &binding.identity == id && &binding.version() == version)
                                .context("cycle member has no frozen native binding")?;
                            dependencies.insert(format!("@source:{id}"), serde_json::json!({"version": version.0,
                                "binding": crate::store::native_tool_results::observers::DependencyBinding::NativeCurrent(binding)}));
                        }
                    }
                    if let EvidenceDependency::SessionActivity { attachment_epoch, activity_revision } = dependency {
                        let payload: String = self.store.lock().query_row("SELECT payload_json FROM native_tool_result_publication
                            WHERE publication_sequence = ?1 AND event_kind = 'session_idle'", [integer(cursor.0)?], |r| r.get(0))?;
                        let host: crate::store::native_tool_results::evidence::HostSessionObservation = serde_json::from_str(&payload)?;
                        ensure!(host.observer.attachment_epoch == *attachment_epoch, "idle epoch changed");
                        dependencies.insert(format!("@activity:{attachment_epoch}"), serde_json::json!({"version": activity_revision.to_string(),
                            "binding": crate::store::native_tool_results::observers::DependencyBinding::SessionActivity {
                                observer: host.observer, activity_revision: *activity_revision }}));
                    }
                }
                ensure!(
                    !dependencies.is_empty(),
                    "candidate has no frozen source dependencies"
                );
                Ok(BurnCheckCandidateRecord {
                    candidate_id: hex(&candidate.fingerprint),
                    check_id: activation.id.definition_id.clone(),
                    activation_scope_key: self.activation_scope_key.clone(),
                    generation: integer(activation.id.generation.0)?,
                    branch_key: candidate.branch.0.clone(),
                    session: session.clone(),
                    trigger_position: integer(candidate.trigger_position.0)?,
                    selected_publication_head: integer(cursor.0)?,
                    dependencies: serde_json::Value::Object(dependencies),
                    frozen_input: serde_json::to_value(candidate)?,
                    input_fingerprint: hex(&candidate.fingerprint),
                    created_at_epoch: now_epoch(),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let expected_json = saved
            .encode()
            .map_err(|error| anyhow::anyhow!(error.message))?;
        let next_json = next
            .encode()
            .map_err(|error| anyhow::anyhow!(error.message))?;
        let applied = self.store.apply_burn_check_publications_with_checkpoint(
            &BurnCheckPublicationPage {
                check_id: &activation.id.definition_id,
                activation_scope_key: &self.activation_scope_key,
                generation: integer(activation.id.generation.0)?,
                through_sequence: integer(page.next.0)?,
                publication_sequences: &sequences,
                observations: &observations,
                candidates: &records,
            },
            Some(RuntimeCheckpoint {
                expected: &expected_json,
                next: &next_json,
                scope: &self.scope,
                invalidated_branches: &invalidated_branches,
                reset_branches: &reset_branches,
                coalesce_pending: activation.compiled.trigger.window.is_some(),
            }),
        )?;
        ensure!(applied, "source page lost activation or publication rights");
        if page.next != expected.publication_cursor || !page.events.is_empty() {
            ::tracing::debug!(
                event = "burn_check_source_page_committed",
                check_ref = check_ref(&activation.id.definition_id),
                activation_ref = activation_ref(&activation.id),
                generation = activation.id.generation.0,
                publication_cursor = page.next.0,
                publication_after_cursor = expected.publication_cursor.0,
                publication_through_cursor = page.next.0,
                publication_origin = "runtime_source_page",
                events = page.events.len(),
                observations = committed.len(),
                candidates = records.len(),
                invalidated_branches = invalidated_branches.len(),
                reset_branches = reset_branches.len(),
                gaps = page
                    .events
                    .iter()
                    .filter(|event| matches!(event, SourceEvent::Gap { .. }))
                    .count(),
                restored = page
                    .events
                    .iter()
                    .filter(|event| matches!(event, SourceEvent::CoverageRestored { .. }))
                    .count(),
                ignored = page
                    .events
                    .iter()
                    .filter(|event| matches!(event, SourceEvent::Ignored { .. }))
                    .count(),
                elapsed_ms = started.elapsed().as_millis() as u64,
            );
        }
        Ok(())
    }
}

fn observation_key(branch: &BranchId, identity: &str) -> String {
    serde_json::to_string(&(&branch.0, identity)).expect("observation identity serializes")
}
