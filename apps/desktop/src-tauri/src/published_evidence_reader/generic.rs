use super::*;
use crate::store::native_tool_results::evidence::{
    EvidencePublication, EvidencePublicationPayload, HostSessionPayload, NativeEvidencePayload,
};
use antiburn_checks::context::ObservedAttribution;
use antiburn_checks::domain::Message;
use antiburn_checks::source::{
    PublishedObservation, PublishedObservationPayload, PublishedResponseCycle,
    PublishedSourceEvidence,
};
use antiburn_local::analysis::{
    NativeEventContext, NativeMessageObservation, NativeToolInput, PublicMessagePayload,
    ToolResultCompleteness, UserMessageChange, UserMessageCompleteness,
};

impl StorePublishedEvidenceReader<'_> {
    pub(super) fn generic_needs_attachment(
        &self,
        publication: &EvidencePublication,
    ) -> Result<bool, SourceError> {
        if !self.request_origin_allowed(publication.publication_sequence)? {
            return Ok(false);
        }
        let Some(activation) = self.activation.filter(|_| self.live) else {
            return Ok(false);
        };
        if crate::store::native_tool_results::admission::event_reason_in(
            &self.store.lock(),
            &publication.session,
            publication.publication_sequence,
        )
        .map_err(|error| self.reader_error(error))?
        .is_some()
        {
            return Ok(false);
        }
        if publication.live_eligible != Some(true)
            || publication.target.is_some()
            || publication.outcome != ToolResultPublicationOutcome::Update
            || publication.event_kind
                == crate::store::native_tool_results::evidence::EvidenceEventKind::ToolResult
                && publication.source_format.as_deref() != Some("\"open_code_v2_sqlite\"")
        {
            return Ok(false);
        }
        let source = crate::store::native_tool_results::opencode_v2_status::SOURCE_SQL;
        self.store
            .lock()
            .query_row(
                &format!("SELECT EXISTS(SELECT 1 FROM session s
            WHERE s.environment_key = ?1 AND s.agent = ?2 AND s.session_id = ?3
               AND s.cwd = json_extract(?5, '$.session_cwd') AND json_extract(?5, '$.run') = 'Live'
               AND (s.source_format IS NOT '\"open_code_v2_sqlite\"' OR (({source}) AND EXISTS(SELECT 1 FROM opencode_v2_capture v WHERE v.environment_key = s.environment_key
                 AND v.agent = s.agent AND v.session_id = s.session_id AND v.incarnation = s.incarnation
                 AND v.source_label = s.source_label AND v.cwd = s.cwd AND v.capture_epoch <> '' AND v.claim_fence > 0)))
              AND NOT EXISTS(SELECT 1 FROM burn_check_session_attachment t
                WHERE t.check_id = ?4 AND t.activation_scope_key = ?5 AND t.generation = ?6
                  AND t.branch_key = ?7 AND t.incarnation = s.incarnation))"),
                rusqlite::params![
                    publication.session.environment_key,
                    publication.session.agent,
                    publication.session.session_id,
                    activation.definition_id,
                    crate::check_store::StoreCheckStore::activation_scope_key(&activation.scope),
                    activation.generation.0,
                    publication.session.to_identity_json()
                ],
                |r| r.get(0),
            )
            .map_err(|error| self.reader_error(error))
    }

    pub(super) fn convert_evidence(
        &self,
        publication: EvidencePublication,
    ) -> Result<PublishedEvidenceEvent, SourceError> {
        let cursor = PublicationCursor(
            u64::try_from(publication.publication_sequence)
                .map_err(|error| self.decode_error(error))?,
        );
        let branch = publication.session.to_identity_json();
        if !self.request_origin_allowed(publication.publication_sequence)? {
            return Ok(PublishedEvidenceEvent {
                cursor,
                branch,
                context: None,
                source_evidence_version: None,
                change: PublishedEvidenceChange::Ignored,
            });
        }
        if publication.outcome == ToolResultPublicationOutcome::Update
            && publication.target.is_none()
        {
            let connection = self.store.lock();
            if let Some(reason) = crate::store::native_tool_results::admission::event_reason_in(
                &connection,
                &publication.session,
                publication.publication_sequence,
            )
            .map_err(|error| self.reader_error(error))?
            {
                return Ok(PublishedEvidenceEvent {
                    cursor,
                    branch,
                    context: None,
                    source_evidence_version: None,
                    change: PublishedEvidenceChange::UnsafeUpdate { reason },
                });
            }
        }
        if publication.event_kind
            == crate::store::native_tool_results::evidence::EvidenceEventKind::ToolResult
        {
            let mut event = self.convert_publication(ToolResultPublication {
                session_selection: publication.session_selection,
                gap_kind: publication.gap_kind,
                publication_sequence: publication.publication_sequence,
                outcome: publication.outcome,
                session: publication.session,
                source_format: publication.source_format,
                native_identity: publication.native_identity,
                native_position: publication.native_position,
                native_revision: publication.native_revision,
                payload: match publication.payload {
                    Some(EvidencePublicationPayload::Native(
                        NativeEvidencePayload::ToolResult(result),
                    )) => Some(result),
                    _ => None,
                },
                complete: publication.complete,
                source_generation: publication.source_generation,
                claim_fence: publication.claim_fence,
                live_eligible: publication.live_eligible,
                gap_reason: publication.gap_reason,
            })?;
            event.source_evidence_version = publication.source_evidence_version;
            return Ok(event);
        }
        if self.live
            && publication.outcome == ToolResultPublicationOutcome::Update
            && publication.live_eligible != Some(true)
        {
            let observed = if let Some(activation) = self.activation {
                self.store.lock().query_row("SELECT EXISTS(SELECT 1 FROM burn_check_observation o
                    JOIN burn_check_activation a USING(check_id, activation_scope_key)
                    WHERE o.check_id = ?1 AND o.activation_scope_key = ?2 AND a.generation = ?3 AND a.enabled = 1
                      AND o.session_key_json = ?4 AND o.native_identity = ?5)", rusqlite::params![activation.definition_id,
                        crate::check_store::StoreCheckStore::activation_scope_key(&activation.scope), activation.generation.0,
                         branch, publication.native_identity], |r| r.get::<_, bool>(0)).map_err(|error| self.reader_error(error))?
            } else {
                false
            };
            if !observed {
                return Ok(PublishedEvidenceEvent {
                    source_evidence_version: None,
                    cursor,
                    branch,
                    context: None,
                    change: PublishedEvidenceChange::Ignored,
                });
            }
        }
        if publication.outcome == ToolResultPublicationOutcome::Removal {
            return Ok(PublishedEvidenceEvent {
                source_evidence_version: None,
                cursor,
                branch,
                context: None,
                change: PublishedEvidenceChange::Removal {
                    identity: publication
                        .native_identity
                        .ok_or_else(|| self.decode_error("removal has no identity"))?,
                    revision: u64::try_from(
                        publication
                            .native_revision
                            .ok_or_else(|| self.decode_error("removal has no revision"))?,
                    )
                    .map_err(|error| self.decode_error(error))?,
                },
            });
        }
        let payload = publication
            .payload
            .as_ref()
            .ok_or_else(|| self.decode_error("evidence has no payload"))?;
        if publication.event_kind
            == crate::store::native_tool_results::evidence::EvidenceEventKind::ResponseCycle
        {
            let connection = self.store.lock();
            for binding in &publication.dependencies {
                if crate::store::native_tool_results::evidence::current_binding_in(
                    &connection,
                    &publication.session,
                    &binding.identity,
                )
                .map_err(|error| self.reader_error(error))?
                .as_ref()
                    != Some(binding)
                {
                    return Ok(PublishedEvidenceEvent {
                        source_evidence_version: None,
                        cursor,
                        branch,
                        context: None,
                        change: PublishedEvidenceChange::Ignored,
                    });
                }
            }
        }
        let (identity, native_order, revision, eligible, complete, validity, payload, context) =
            match payload {
                EvidencePublicationPayload::Host(host) => {
                    let identity = serde_json::to_string(&(
                        publication.event_kind.name(),
                        &host.observer,
                        &host.payload,
                    ))
                    .map_err(|error| self.decode_error(error))?;
                    let payload = match &host.payload {
                        HostSessionPayload::Start { reason } => {
                            PublishedObservationPayload::SessionStart { reason: *reason }
                        }
                        HostSessionPayload::Idle {
                            activity_revision,
                            idle_seconds,
                            last_activity_at_ms,
                        } => PublishedObservationPayload::SessionIdle {
                            attachment_epoch: host.observer.attachment_epoch.clone(),
                            activity_revision: *activity_revision,
                            idle_seconds: *idle_seconds,
                            last_activity_at_ms: *last_activity_at_ms,
                        },
                    };
                    let mut context = host.context.clone();
                    context.project_root = self.repository_root.map(str::to_owned);
                    (
                        identity,
                        NativeOrderKey(
                            serde_json::to_string(&(
                                &host.observer,
                                publication.publication_sequence,
                            ))
                            .map_err(|error| self.decode_error(error))?,
                        ),
                        1,
                        true,
                        true,
                        EvidenceValidity::Valid,
                        payload,
                        context,
                    )
                }
                EvidencePublicationPayload::Native(native) => {
                    let identity = native
                        .identity()
                        .map_err(|error| self.decode_error(error))?;
                    let context = match native {
                        NativeEvidencePayload::UserMessage(m) => Some(&m.context),
                        NativeEvidencePayload::PublicMessage(m) => m.context.as_ref(),
                        NativeEvidencePayload::ResponseCycle(m) => Some(&m.context),
                        NativeEvidencePayload::ToolResult(_) => unreachable!(),
                    }
                    .ok_or_else(|| self.decode_error("public evidence lacks context"))?;
                    let context = public_context(
                        self,
                        context,
                        &publication.session,
                        self.repository_root,
                        publication.session_selection.clone(),
                    )?;
                    let (payload, eligible, complete, validity) = match native {
                        NativeEvidencePayload::UserMessage(m) => {
                            let complete =
                                m.message.completeness == UserMessageCompleteness::Complete;
                            (
                                PublishedObservationPayload::UserMessage(public_message(
                                    self,
                                    &NativeMessageObservation::User(m.clone()),
                                    &publication.session,
                                    self.repository_root,
                                )?),
                                m.message.change == UserMessageChange::Present,
                                complete,
                                if m.message.is_bounded()
                                    && m.message.change != UserMessageChange::Conflict
                                {
                                    EvidenceValidity::Valid
                                } else {
                                    EvidenceValidity::UnsafeIdentity
                                },
                            )
                        }
                        NativeEvidencePayload::PublicMessage(m) => {
                            let message = public_message(
                                self,
                                &NativeMessageObservation::Public(m.clone()),
                                &publication.session,
                                self.repository_root,
                            )?;
                            (
                                match m.payload {
                                    PublicMessagePayload::AssistantText { .. } => {
                                        PublishedObservationPayload::AssistantMessage(message)
                                    }
                                    PublicMessagePayload::ToolCall { .. } => {
                                        PublishedObservationPayload::ToolCall(message)
                                    }
                                },
                                m.eligible,
                                m.completeness == ToolResultCompleteness::Complete,
                                validity(m.completeness),
                            )
                        }
                        NativeEvidencePayload::ResponseCycle(m) => (
                            PublishedObservationPayload::ResponseCycle(PublishedResponseCycle {
                                id: identity.clone(),
                                status: match m.status {
                                    antiburn_local::analysis::ResponseCycleStatus::Completed => {
                                        antiburn_checks::source::ResponseCycleStatus::Completed
                                    }
                                    antiburn_local::analysis::ResponseCycleStatus::Cancelled => {
                                        antiburn_checks::source::ResponseCycleStatus::Cancelled
                                    }
                                    antiburn_local::analysis::ResponseCycleStatus::Error => {
                                        antiburn_checks::source::ResponseCycleStatus::Error
                                    }
                                },
                                messages: m
                                    .messages
                                    .iter()
                                    .map(|m| {
                                        public_message(
                                            self,
                                            m,
                                            &publication.session,
                                            self.repository_root,
                                        )
                                    })
                                    .collect::<Result<_, _>>()?,
                                attribution: attribution(&m.requester),
                                native_reason: m.native_reason.clone(),
                                native_error_code: m.native_error_code.clone(),
                                started_at_ms: m.started_at_ms,
                                ended_at_ms: m.ended_at_ms,
                                dependencies: publication
                                    .dependencies
                                    .iter()
                                    .map(|b| PublishedSourceEvidence {
                                        identity: b.identity.clone(),
                                        version: b.version(),
                                    })
                                    .collect(),
                            }),
                            true,
                            true,
                            EvidenceValidity::Valid,
                        ),
                        NativeEvidencePayload::ToolResult(_) => unreachable!(),
                    };
                    let position = match native {
                        NativeEvidencePayload::UserMessage(m) => m.context.first_eligible_position,
                        NativeEvidencePayload::PublicMessage(m) => m
                            .context
                            .as_ref()
                            .map_or(m.position, |c| c.first_eligible_position),
                        NativeEvidencePayload::ResponseCycle(m) => {
                            m.context.first_eligible_position
                        }
                        _ => native.position(),
                    };
                    (
                        identity,
                        NativeOrderKey(
                            serde_json::to_string(&position)
                                .map_err(|error| self.decode_error(error))?,
                        ),
                        native.revision(),
                        eligible,
                        complete,
                        validity,
                        payload,
                        context,
                    )
                }
            };
        Ok(PublishedEvidenceEvent {
            source_evidence_version: publication.source_evidence_version,
            cursor,
            branch,
            context: Some(context),
            change: PublishedEvidenceChange::ObservationUpsert(PublishedObservation {
                identity,
                native_order,
                revision,
                eligible,
                payload_complete: complete,
                validity,
                payload,
                first_eligible_publication: publication
                    .first_eligible_publication
                    .map(|n| u64::try_from(n).map(PublicationCursor))
                    .transpose()
                    .map_err(|error| self.decode_error(error))?
                    .or_else(|| publication.target.as_ref().map(|_| cursor)),
            }),
        })
    }
}

fn validity(complete: ToolResultCompleteness) -> EvidenceValidity {
    match complete {
        ToolResultCompleteness::Complete | ToolResultCompleteness::Incomplete => {
            EvidenceValidity::Valid
        }
        ToolResultCompleteness::UnsafeIdentity => EvidenceValidity::UnsafeIdentity,
        ToolResultCompleteness::UnsafeOrder => EvidenceValidity::UnsafeOrder,
        ToolResultCompleteness::UnsafeOwnership => EvidenceValidity::UnsafeOwnership,
    }
}

fn attribution(native: &antiburn_local::analysis::NativeRequestAttribution) -> ObservedAttribution {
    ObservedAttribution {
        model: native.model_key(),
        fast_mode: native.fast_mode,
    }
}

fn public_context(
    reader: &StorePublishedEvidenceReader<'_>,
    context: &NativeEventContext,
    session: &SessionKey,
    root: Option<&str>,
    selection: Option<ObservedAttribution>,
) -> Result<PublishedContext, SourceError> {
    published_context(
        reader,
        &antiburn_local::analysis::ToolResultContext {
            result_position: context.first_eligible_position,
            main_session: context.main_session,
            complete_through_event: context.complete_through_event,
            user_messages: context.user_messages.clone(),
        },
        session,
        root,
        selection,
    )
}

fn public_message(
    reader: &StorePublishedEvidenceReader<'_>,
    message: &NativeMessageObservation,
    session: &SessionKey,
    root: Option<&str>,
) -> Result<Message, SourceError> {
    let identity = crate::store::native_tool_results::evidence::message_payload(message)
        .identity()
        .map_err(|error| reader.decode_error(error))?;
    match message {
        NativeMessageObservation::User(m) => antiburn_checks::source::normalize_user_message(
            &identity,
            &session.agent,
            &m.message
                .parts
                .iter()
                .map(|p| p.text.as_str())
                .collect::<Vec<_>>()
                .join("\n"),
            m.message.completeness == UserMessageCompleteness::Complete,
        ),
        NativeMessageObservation::Public(m) => match &m.payload {
            PublicMessagePayload::AssistantText { parts, generator } => {
                antiburn_checks::source::normalize_assistant_message(
                    &identity,
                    &session.agent,
                    &parts
                        .iter()
                        .map(|p| p.text.as_str())
                        .collect::<Vec<_>>()
                        .join("\n"),
                    m.completeness == ToolResultCompleteness::Complete,
                    &attribution(generator),
                )
            }
            PublicMessagePayload::ToolCall {
                tool_name,
                input,
                facts,
                provider_executed,
            } => antiburn_checks::source::normalize_tool_call(
                &identity,
                &session.agent,
                m.call_id
                    .as_deref()
                    .or(m.part_id.as_deref())
                    .unwrap_or(&m.record_id),
                tool_name,
                input.as_ref().map(|input| match input {
                    NativeToolInput::Json(v) => v.clone(),
                    NativeToolInput::Text(s) => serde_json::Value::String(s.clone()),
                }),
                &published_tool_metadata(facts.as_ref(), m.source_format, root),
                *provider_executed,
                m.completeness == ToolResultCompleteness::Complete,
            ),
        },
        NativeMessageObservation::ToolResult(m) => {
            let event = publication_event(
                reader,
                ToolResultPublication {
                    session_selection: None,
                    gap_kind: PublicationGapKind::Invalidated,
                    publication_sequence: 1,
                    outcome: ToolResultPublicationOutcome::Update,
                    session: session.clone(),
                    source_format: None,
                    native_identity: Some(identity),
                    native_position: Some(m.position),
                    native_revision: Some(
                        i64::try_from(m.revision).map_err(|error| reader.decode_error(error))?,
                    ),
                    payload: Some(m.clone()),
                    complete: true,
                    source_generation: 0,
                    claim_fence: 0,
                    live_eligible: Some(true),
                    gap_reason: None,
                },
                root,
                None,
            )?;
            let PublishedEvidenceChange::Upsert(result) = event.change else {
                unreachable!()
            };
            antiburn_checks::source::normalize_tool_result_message(
                &result,
                &event
                    .context
                    .ok_or_else(|| reader.decode_error("cycle result lacks context"))?,
            )
        }
    }
}
