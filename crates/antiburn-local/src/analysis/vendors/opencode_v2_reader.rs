//! Normalizes the bounded facts in OpenCode V2 snapshot messages.

use anyhow::Context;
use rusqlite::{Connection, OpenFlags};
use serde_json::Value;
use std::sync::Arc;

#[path = "opencode_v2_tools.rs"]
mod tools;

use super::opencode_v2_snapshot::{
    MAX_SNAPSHOT_ROWS, MAX_SNAPSHOT_TOTAL_BYTES, SnapshotMessage, SnapshotSession,
    session_cluster_ids, visit_snapshot_connection,
};
use crate::analysis::jev::JevInputField;
use crate::analysis::jev_evidence::{
    JevNativeFieldContainer, JevNativeFieldRange, JevNativeSkillBody, JevNativeSkillOrigin,
    JevNativeSkillProducer, JevNativeSkillProof, JevRecordedSkillResult,
};
use crate::analysis::model::CompactionTrigger;
use crate::analysis::{
    CalculatedCost, ContentAuthority, ContentKind, ContentPart, EVIDENCE_STRING_CAP, EventSource,
    EvidenceObservation, MAX_CONTENT_PART_BYTES, MAX_USER_MESSAGE_PARTS, NormalizedRecord,
    NormalizedSession, PartialReason, ProviderHint, RawSource, RecordSink, SessionCollector,
    SessionInput, SessionReader, SessionSummary, SourceAccounting, SourceCapabilities,
    SourceChangedReason, SourceFormat, StepAccounting, TokenClasses, TurnContent,
    UsageReconciliation, VisitOutcome,
};
use crate::analysis::{NormalizedEvent, Role};

const MAX_TRANSFER_BYTES: usize = 16 * 1024 * 1024;
const MAX_TRANSFER_MESSAGES: usize = 100_000;

pub struct OpenCodeV2SessionReader;

impl SessionReader for OpenCodeV2SessionReader {
    fn agent(&self) -> &'static str {
        "opencode"
    }

    fn capabilities(&self, input: &SessionInput) -> SourceCapabilities {
        match input.source_format {
            SourceFormat::OpenCodeSqliteV2 => SourceCapabilities::opencode_v2(),
            _ => SourceCapabilities::uncharacterized(input.source_format),
        }
    }

    fn normalize(&self, input: &SessionInput) -> anyhow::Result<NormalizedSession> {
        let mut collector = SessionCollector::new(input.agent.clone(), input.session_id.clone());
        self.visit(input, &mut collector)?;
        collector.into_session()
    }

    fn visit(
        &self,
        input: &SessionInput,
        sink: &mut dyn RecordSink,
    ) -> anyhow::Result<VisitOutcome> {
        visit_with_cancel(input, &|| false, sink)
    }

    fn visit_db_claimed(
        &self,
        input: &SessionInput,
        claimed_fingerprint: &str,
        cancel: &dyn Fn() -> bool,
        sink: &mut dyn RecordSink,
    ) -> anyhow::Result<VisitOutcome> {
        visit_database(input, Some(claimed_fingerprint), cancel, sink)
    }
}

fn visit_database(
    input: &SessionInput,
    claimed: Option<&str>,
    cancel: &dyn Fn() -> bool,
    sink: &mut dyn RecordSink,
) -> anyhow::Result<VisitOutcome> {
    let (SourceFormat::OpenCodeSqliteV2, RawSource::Sqlite(path)) =
        (input.source_format, &input.source)
    else {
        anyhow::bail!("a claimed OpenCode V2 database source must be V2 SQLite");
    };
    if cancel() {
        anyhow::bail!("OpenCode V2 snapshot read was cancelled");
    }
    let file = std::fs::File::open(path)?;
    let database_identity = crate::discovery::SourceStat::from_open_std_file(&file)
        .and_then(|stat| stat.identity)
        .context("OpenCode V2 database identity is unavailable")?;
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    let opened_file = std::fs::File::open(path)?;
    let opened_identity = crate::discovery::SourceStat::from_open_std_file(&opened_file)
        .and_then(|stat| stat.identity)
        .context("OpenCode V2 database identity is unavailable")?;
    if opened_identity != database_identity {
        return Ok(VisitOutcome::SourceChanged(
            SourceChangedReason::FingerprintMismatch,
        ));
    }
    connection.execute_batch("BEGIN")?;
    if let Some(claimed) = claimed {
        let actual =
            crate::discovery::agents::opencode::db_session_fingerprint_cancellable_with_identity(
                &connection,
                &input.session_id,
                &database_identity,
                cancel,
            )
            .map(|(latest, rows)| {
                crate::discovery::source_version::provider_db_fingerprint(latest, rows)
            });
        if cancel() {
            anyhow::bail!("OpenCode V2 snapshot read was cancelled");
        }
        if actual.as_deref() != Some(claimed) {
            return Ok(VisitOutcome::SourceChanged(
                SourceChangedReason::FingerprintMismatch,
            ));
        }
    }
    let mut state = ReaderState::default();
    let mut rows_remaining = MAX_SNAPSHOT_ROWS;
    let mut bytes_remaining = MAX_SNAPSHOT_TOTAL_BYTES;
    for id in session_cluster_ids(&connection, &input.session_id)? {
        if cancel() {
            anyhow::bail!("OpenCode V2 snapshot read was cancelled");
        }
        let mut source_state = ReaderState {
            delegated: id != input.session_id,
            ..ReaderState::default()
        };
        sink.begin_source(&id);
        let snapshot = visit_snapshot_connection(
            &connection,
            &id,
            cancel,
            &mut |message| {
                source_state.observe(message, sink);
                Ok(())
            },
            rows_remaining,
            bytes_remaining,
        )
        .context("reading OpenCode V2 session snapshot")?;
        rows_remaining = rows_remaining.saturating_sub(snapshot.rows_seen);
        bytes_remaining = bytes_remaining.saturating_sub(snapshot.bytes_visited);
        if snapshot.is_partial() {
            if snapshot.oversized_rows > 0 {
                source_state.partial(PartialReason::Oversized, sink);
            }
            if snapshot.malformed_rows > 0 {
                source_state.partial(PartialReason::MalformedRecord, sink);
            }
        }
        source_state.reconcile(&snapshot.session, sink);
        if snapshot.capacity_limit_reached {
            source_state.partial(PartialReason::CapacityExceeded, sink);
        }
        sink.source_summary(&id, source_state.finish());
        state.include(&source_state);
        if snapshot.capacity_limit_reached {
            break;
        }
    }
    if cancel() {
        anyhow::bail!("OpenCode V2 snapshot read was cancelled");
    }
    let current_file = std::fs::File::open(path)?;
    let current_identity = crate::discovery::SourceStat::from_open_std_file(&current_file)
        .and_then(|stat| stat.identity)
        .context("OpenCode V2 database identity is unavailable")?;
    if current_identity != database_identity {
        return Ok(VisitOutcome::SourceChanged(
            SourceChangedReason::FingerprintMismatch,
        ));
    }
    connection.execute_batch("COMMIT")?;
    sink.tool_result_gap();
    sink.finish(state.finish());
    Ok(if claimed.is_some() {
        VisitOutcome::AcceptedFull
    } else {
        VisitOutcome::Unvalidated
    })
}

fn visit_with_cancel(
    input: &SessionInput,
    cancel: &dyn Fn() -> bool,
    sink: &mut dyn RecordSink,
) -> anyhow::Result<VisitOutcome> {
    let mut state = ReaderState::default();
    match (input.source_format, &input.source) {
        (SourceFormat::OpenCodeSqliteV2, RawSource::Sqlite(_)) => {
            return visit_database(input, None, cancel, sink);
        }
        (SourceFormat::OpenCodeV2SessionExport, RawSource::Jsonl(content)) => {
            visit_transfer(content, input, cancel, &mut state, sink)?;
        }
        _ => anyhow::bail!(
            "OpenCode V2 source format {:?} does not match its source",
            input.source_format
        ),
    }
    let summary = state.finish();
    sink.tool_result_gap();
    sink.finish(summary);
    Ok(VisitOutcome::Unvalidated)
}

fn visit_transfer(
    content: &str,
    input: &SessionInput,
    cancel: &dyn Fn() -> bool,
    state: &mut ReaderState,
    sink: &mut dyn RecordSink,
) -> anyhow::Result<()> {
    if content.len() > MAX_TRANSFER_BYTES {
        state.partial(PartialReason::Oversized, sink);
        return Ok(());
    }
    let transfer: Value = match serde_json::from_str(content) {
        Ok(value) => value,
        Err(_) => {
            state.partial(PartialReason::MalformedRecord, sink);
            return Ok(());
        }
    };
    let info = transfer.get("info").and_then(Value::as_object);
    let messages = transfer.get("messages").and_then(Value::as_array);
    if info.and_then(|info| info.get("id")).and_then(Value::as_str)
        != Some(input.session_id.as_str())
    {
        state.partial(PartialReason::AttributionIncomplete, sink);
        return Ok(());
    }
    let Some(messages) = messages else {
        state.partial(PartialReason::MalformedRecord, sink);
        return Ok(());
    };
    let session = Arc::new(SnapshotSession {
        transfer: true,
        parent_id: transfer
            .pointer("/info/parentID")
            .and_then(Value::as_str)
            .map(str::to_owned),
        fork_session_id: transfer
            .pointer("/info/fork/sessionID")
            .and_then(Value::as_str)
            .map(str::to_owned),
        fork_boundary: transfer.pointer("/info/fork/boundary").cloned(),
        unresolved_fork: transfer.pointer("/info/fork").is_some(),
        tokens: transfer.pointer("/info/tokens").cloned(),
        calculated_cost: transfer.pointer("/info/cost").and_then(Value::as_f64),
        accounting_malformed: transfer
            .pointer("/info/cost")
            .is_some_and(|cost| cost.as_f64().is_none()),
        ..SnapshotSession::default()
    });
    for (index, message) in messages.iter().take(MAX_TRANSFER_MESSAGES).enumerate() {
        if cancel() {
            anyhow::bail!("OpenCode V2 transfer read was cancelled");
        }
        let Some(object) = message.as_object() else {
            state.partial(PartialReason::MalformedRecord, sink);
            continue;
        };
        state.observe(
            SnapshotMessage {
                session_id: input.session_id.clone(),
                seq: index as i64,
                value: Value::Object(object.clone()),
                session: Arc::clone(&session),
            },
            sink,
        );
    }
    if messages.len() > MAX_TRANSFER_MESSAGES {
        state.partial(PartialReason::Oversized, sink);
    }
    state.reconcile(&session, sink);
    Ok(())
}

struct ReaderState {
    model: Option<String>,
    assistant_messages: u64,
    cache_write_complete: bool,
    coverage_gaps: Vec<PartialReason>,
    provider_hints: Vec<ProviderHint>,
    accounting: SourceAccounting,
    included_sources: usize,
    delegated: bool,
    launched_children: std::collections::HashSet<String>,
}

impl Default for ReaderState {
    fn default() -> Self {
        Self {
            model: None,
            assistant_messages: 0,
            cache_write_complete: true,
            coverage_gaps: Vec::new(),
            provider_hints: Vec::new(),
            accounting: SourceAccounting::default(),
            included_sources: 0,
            delegated: false,
            launched_children: std::collections::HashSet::new(),
        }
    }
}

impl ReaderState {
    fn include(&mut self, source: &Self) {
        self.accounting = if self.included_sources == 0 {
            source.accounting
        } else {
            self.accounting.checked_merge(source.accounting)
        };
        self.included_sources += 1;
        self.assistant_messages = self
            .assistant_messages
            .saturating_add(source.assistant_messages);
        self.cache_write_complete &= source.cache_write_complete;
        if !source.delegated {
            self.model.clone_from(&source.model);
        }
        for reason in &source.coverage_gaps {
            if !self.coverage_gaps.contains(reason) {
                self.coverage_gaps.push(*reason);
            }
        }
        for hint in &source.provider_hints {
            if !self.provider_hints.contains(hint) && self.provider_hints.len() < 32 {
                self.provider_hints.push(hint.clone());
            }
        }
    }

    fn observe(&mut self, message: SnapshotMessage, sink: &mut dyn RecordSink) {
        if message.session.unresolved_fork {
            self.partial(PartialReason::AttributionIncomplete, sink);
            return;
        }
        if message
            .session
            .inherited_through
            .is_some_and(|boundary| message.seq <= boundary)
        {
            sink.record(NormalizedRecord::Observation(Box::new(
                EvidenceObservation::InheritedRecord,
            )));
            return;
        }
        let value = &message.value;
        let Some(message_id) = value.get("id").and_then(Value::as_str).map(str::to_owned) else {
            self.partial(PartialReason::MalformedRecord, sink);
            return;
        };
        if message_id.is_empty() || message_id.len() > EVIDENCE_STRING_CAP {
            self.partial(PartialReason::MalformedRecord, sink);
            return;
        }
        let Some(message_type) = value.get("type").and_then(Value::as_str) else {
            self.partial(PartialReason::MalformedRecord, sink);
            return;
        };
        match message_type {
            "assistant" => self.observe_assistant(message, &message_id, sink),
            "user" => self.observe_user(message, &message_id, sink),
            "synthetic" | "system" | "skill" => self.observe_context(message, &message_id, sink),
            "shell" => self.observe_shell(message, &message_id, sink),
            "compaction" => self.observe_compaction(message, &message_id, sink),
            "agent-switched" | "model-switched" | "location-switched" | "idle" => {
                let field = match message_type {
                    "agent-switched" => value.get("agent").and_then(Value::as_str).is_some(),
                    "model-switched" => model_ref(value.get("model")).is_some(),
                    "location-switched" => value
                        .pointer("/location/directory")
                        .and_then(Value::as_str)
                        .is_some(),
                    _ => matches!(
                        value.get("outcome").and_then(Value::as_str),
                        Some("succeeded" | "failed" | "interrupted")
                    ),
                };
                if let Some(event) = base_event(value, Role::System) {
                    sink.record(NormalizedRecord::Observation(Box::new(
                        EvidenceObservation::RecordTimestamp {
                            ts_ms: event.ts_ms.unwrap(),
                        },
                    )));
                } else {
                    self.partial(PartialReason::MalformedRecord, sink);
                }
                if !field {
                    self.partial(PartialReason::MalformedRecord, sink);
                }
            }
            _ => self.partial(PartialReason::MalformedRecord, sink),
        }
    }

    fn observe_user(
        &mut self,
        message: SnapshotMessage,
        message_id: &str,
        sink: &mut dyn RecordSink,
    ) {
        let value = &message.value;
        let Some(mut event) = base_event(value, Role::User) else {
            self.partial(PartialReason::MalformedRecord, sink);
            return;
        };
        event.uuid = Some(message_id.to_owned());
        event.thread_id = Some(message.session_id.clone());
        let Some(text) = value.get("text").and_then(Value::as_str) else {
            self.partial(PartialReason::MalformedRecord, sink);
            self.record_event(event, sink);
            return;
        };
        let mut captured = CapturedContent::default();
        captured.push(ContentKind::UserText, text, None, None);
        match value.get("files") {
            Some(Value::Array(files)) if !files.is_empty() => {
                self.partial(PartialReason::UnrecognizedRecordType, sink);
            }
            Some(Value::Array(_)) | None => {}
            Some(_) => self.partial(PartialReason::MalformedRecord, sink),
        }
        if let Some(skills) = value.get("skills") {
            match skills.as_array() {
                Some(skills) => {
                    for (index, skill) in skills.iter().take(64).enumerate() {
                        let id = skill
                            .get("id")
                            .and_then(Value::as_str)
                            .filter(|id| tools::valid_skill_name(id));
                        let Ok(index) = u32::try_from(index) else {
                            self.partial(PartialReason::CapacityExceeded, sink);
                            break;
                        };
                        if !capture_injected_skill(
                            skill,
                            JevNativeSkillOrigin::PromptAttachment { index },
                            &message,
                            message_id,
                            &mut captured,
                        ) {
                            self.partial(PartialReason::AttributionIncomplete, sink);
                        } else if let Some(id) = id {
                            sink.record(NormalizedRecord::Observation(Box::new(
                                EvidenceObservation::SkillInjection {
                                    name: id.to_owned(),
                                    invoked: true,
                                },
                            )));
                        }
                    }
                    if skills.len() > 64 {
                        captured.oversized = true;
                    }
                }
                None => self.partial(PartialReason::MalformedRecord, sink),
            }
        }
        if captured.oversized {
            self.partial(PartialReason::Oversized, sink);
        }
        if captured.incomplete {
            self.partial(PartialReason::IncompleteTail, sink);
        }
        self.record_event(event, sink);
        if !captured.parts.is_empty() {
            sink.record(NormalizedRecord::TurnContent(Box::new(TurnContent {
                parts: captured.parts,
            })));
        }
    }

    fn observe_assistant(
        &mut self,
        message: SnapshotMessage,
        message_id: &str,
        sink: &mut dyn RecordSink,
    ) {
        let value = &message.value;
        let Some(mut event) = base_event(value, Role::Assistant) else {
            self.partial(PartialReason::MalformedRecord, sink);
            return;
        };
        let Some((model_id, provider_id)) = model_ref(value.get("model")) else {
            self.partial(PartialReason::MalformedRecord, sink);
            return;
        };
        event.model = Some(model_id.to_owned());
        event.provider = Some(provider_id.to_owned());
        event.uuid = Some(message_id.to_owned());
        event.thread_id = Some(message.session_id.clone());

        self.observe_usage(value, &mut event, sink);
        let hint = ProviderHint {
            provider: provider_id.to_owned(),
            model: Some(model_id.to_owned()),
        };
        if !self.provider_hints.contains(&hint) && self.provider_hints.len() < 32 {
            self.provider_hints.push(hint);
        }
        self.assistant_messages = self.assistant_messages.saturating_add(1);
        if self.model.is_none() {
            self.model = Some(model_id.to_owned());
        }

        let Some(contents) = value.get("content").and_then(Value::as_array) else {
            self.partial(PartialReason::MalformedRecord, sink);
            self.record_event(event, sink);
            return;
        };
        let mut captured = CapturedContent::default();
        let mut call_ids = std::collections::HashSet::new();
        for (index, content) in contents.iter().take(MAX_USER_MESSAGE_PARTS).enumerate() {
            match content.get("type").and_then(Value::as_str) {
                Some("text") => {
                    if !capture_assistant_text(content, &mut captured) {
                        self.partial(PartialReason::MalformedRecord, sink);
                    }
                }
                Some("reasoning") => {
                    if content.get("text").and_then(Value::as_str).is_none() {
                        self.partial(PartialReason::MalformedRecord, sink);
                    }
                    event.has_thinking = true;
                }
                Some("tool") => {
                    if let Some(id) = content.get("id").and_then(Value::as_str)
                        && !call_ids.insert(id)
                    {
                        self.partial(PartialReason::MalformedRecord, sink);
                        continue;
                    }
                    if !tools::capture_tool(content, index, &message, &mut event, &mut captured) {
                        self.partial(PartialReason::MalformedRecord, sink);
                    }
                }
                _ => self.partial(PartialReason::MalformedRecord, sink),
            }
        }
        if contents.len() > MAX_USER_MESSAGE_PARTS {
            captured.oversized = true;
        }
        if captured.omitted_non_text {
            self.partial(PartialReason::UnrecognizedRecordType, sink);
        }
        if captured.oversized {
            self.partial(PartialReason::Oversized, sink);
        }
        if captured.incomplete {
            self.partial(PartialReason::IncompleteTail, sink);
        }
        tools::observe_tool_relations(&message, &mut event, &mut self.launched_children, sink);

        self.record_event(event, sink);
        if !captured.parts.is_empty() {
            sink.record(NormalizedRecord::TurnContent(Box::new(TurnContent {
                parts: captured.parts,
            })));
        }
    }

    fn partial(&mut self, reason: PartialReason, sink: &mut dyn RecordSink) {
        self.cache_write_complete = false;
        if !self.coverage_gaps.contains(&reason) {
            self.coverage_gaps.push(reason);
            sink.record(NormalizedRecord::Unusable(reason));
        }
    }

    fn record_event(&self, mut event: NormalizedEvent, sink: &mut dyn RecordSink) {
        if self.delegated {
            event.source = EventSource::Subagent;
        }
        sink.record(NormalizedRecord::MetricsEvent(Box::new(event)));
    }

    fn observe_usage(
        &mut self,
        value: &Value,
        event: &mut NormalizedEvent,
        sink: &mut dyn RecordSink,
    ) {
        let mut accounting = StepAccounting::default();
        if let Some(variant) = value.pointer("/model/variant") {
            match variant
                .as_str()
                .filter(|variant| variant.len() <= EVIDENCE_STRING_CAP && !variant.contains('\0'))
            {
                Some(variant) => accounting.native_variant = Some(variant.to_owned()),
                None => self.partial(PartialReason::MalformedRecord, sink),
            }
        }
        match value.get("tokens") {
            Some(tokens) => match TokenClasses::parse(tokens)
                .and_then(|tokens| tokens.normalized_usage().map(|usage| (tokens, usage)))
            {
                Some((tokens, usage)) => {
                    accounting.token_classes = Some(tokens);
                    event.usage = usage;
                }
                None => {
                    self.cache_write_complete = false;
                    self.partial(PartialReason::MalformedRecord, sink);
                }
            },
            None => self.cache_write_complete = false,
        }
        if let Some(cost) = value.get("cost") {
            if let Some(cost) = cost.as_f64().and_then(CalculatedCost::producer_calculated) {
                accounting.calculated_cost = Some(cost);
            } else {
                self.partial(PartialReason::MalformedRecord, sink);
            }
        }
        self.accounting.observe(&accounting);
        if self.accounting.retained_tokens.is_none()
            || self.accounting.retained_calculated_cost.is_none()
        {
            self.partial(PartialReason::CapacityExceeded, sink);
        }
        event.accounting = Some(accounting);
    }

    fn reconcile(&mut self, session: &SnapshotSession, sink: &mut dyn RecordSink) {
        if session.accounting_malformed {
            self.partial(PartialReason::MalformedRecord, sink);
        }
        if session.unresolved_fork {
            self.partial(PartialReason::AttributionIncomplete, sink);
        }
        if let Some(tokens) = &session.tokens {
            match TokenClasses::parse(tokens) {
                Some(tokens) => self.accounting.aggregate_tokens = Some(tokens),
                None => self.partial(PartialReason::MalformedRecord, sink),
            }
        }
        if let Some(cost) = session.calculated_cost {
            match CalculatedCost::producer_calculated(cost) {
                Some(cost) => self.accounting.aggregate_calculated_cost = Some(cost),
                None => self.partial(PartialReason::MalformedRecord, sink),
            }
        }
        if matches!(
            self.accounting.token_reconciliation(),
            UsageReconciliation::Unallocated(_) | UsageReconciliation::Inconsistent
        ) || matches!(
            self.accounting.cost_reconciliation(),
            UsageReconciliation::Unallocated(_) | UsageReconciliation::Inconsistent
        ) {
            self.partial(PartialReason::AttributionIncomplete, sink);
        }
    }

    fn observe_context(
        &mut self,
        message: SnapshotMessage,
        message_id: &str,
        sink: &mut dyn RecordSink,
    ) {
        let value = &message.value;
        let Some(mut event) = base_event(value, Role::System) else {
            self.partial(PartialReason::MalformedRecord, sink);
            return;
        };
        event.uuid = Some(message_id.to_owned());
        event.thread_id = Some(message.session_id.clone());
        let mut captured = CapturedContent::default();
        if value["type"] == "skill" {
            if !capture_injected_skill(
                value,
                JevNativeSkillOrigin::SkillMessage,
                &message,
                message_id,
                &mut captured,
            ) {
                self.partial(PartialReason::AttributionIncomplete, sink);
            }
        } else if let Some(text) = value.get("text").and_then(Value::as_str) {
            captured.push(ContentKind::AssistantText, text, None, None);
            if let Some(part) = captured.parts.last_mut() {
                part.authority = ContentAuthority::System;
            }
        } else {
            self.partial(PartialReason::MalformedRecord, sink);
        }
        if value["type"] == "skill"
            && captured
                .parts
                .iter()
                .any(|part| part.metadata.recorded_skill_result.is_some())
            && let Some(id) = value
                .get("skill")
                .and_then(Value::as_str)
                .filter(|id| tools::valid_skill_name(id))
        {
            sink.record(NormalizedRecord::Observation(Box::new(
                EvidenceObservation::SkillInjection {
                    name: id.to_owned(),
                    invoked: true,
                },
            )));
        }
        if captured.oversized {
            self.partial(PartialReason::Oversized, sink);
        }
        if captured.incomplete {
            self.partial(PartialReason::IncompleteTail, sink);
        }
        self.record_event(event, sink);
        if !captured.parts.is_empty() {
            sink.record(NormalizedRecord::TurnContent(Box::new(TurnContent {
                parts: captured.parts,
            })));
        }
    }

    fn observe_compaction(
        &mut self,
        message: SnapshotMessage,
        message_id: &str,
        sink: &mut dyn RecordSink,
    ) {
        let value = &message.value;
        let Some(mut event) = base_event(value, Role::System) else {
            self.partial(PartialReason::MalformedRecord, sink);
            return;
        };
        event.uuid = Some(message_id.to_owned());
        event.thread_id = Some(message.session_id);
        let status = value.get("status").and_then(Value::as_str);
        if !matches!(status, Some("running" | "completed" | "failed")) {
            self.partial(PartialReason::MalformedRecord, sink);
        }
        event.is_compaction_boundary = status == Some("completed");
        event.compaction_trigger = match value.get("reason").and_then(Value::as_str) {
            Some("auto") => Some(CompactionTrigger::Auto),
            Some("manual") => Some(CompactionTrigger::Manual),
            _ => {
                self.partial(PartialReason::MalformedRecord, sink);
                None
            }
        };
        if !event.is_compaction_boundary {
            event.compaction_trigger = None;
        }
        if let Some((model, provider)) = model_ref(value.get("model")) {
            event.model = Some(model.to_owned());
            event.provider = Some(provider.to_owned());
        } else if value.get("tokens").is_some() {
            self.partial(PartialReason::AttributionIncomplete, sink);
        }
        if status != Some("running") {
            self.observe_usage(value, &mut event, sink);
        }
        // A checkpoint can include opaque provider state. Its text is not a user request.
        self.record_event(event, sink);
    }

    fn observe_shell(
        &mut self,
        message: SnapshotMessage,
        message_id: &str,
        sink: &mut dyn RecordSink,
    ) {
        let value = &message.value;
        let Some(mut event) = base_event(value, Role::Tool) else {
            self.partial(PartialReason::MalformedRecord, sink);
            return;
        };
        event.uuid = Some(message_id.to_owned());
        event.thread_id = Some(message.session_id.clone());
        let mut captured = CapturedContent::default();
        if !tools::capture_shell_message(value, &mut captured) {
            self.partial(PartialReason::MalformedRecord, sink);
        }
        if captured.oversized {
            self.partial(PartialReason::Oversized, sink);
        }
        if captured.incomplete {
            self.partial(PartialReason::IncompleteTail, sink);
        }
        self.record_event(event, sink);
        if !captured.parts.is_empty() {
            sink.record(NormalizedRecord::TurnContent(Box::new(TurnContent {
                parts: captured.parts,
            })));
        }
    }

    fn finish(&self) -> SessionSummary {
        SessionSummary {
            accounting: Some(self.accounting),
            cache_write_tokens_available: self.assistant_messages > 0 && self.cache_write_complete,
            model: self.model.clone(),
            coverage_gaps: self.coverage_gaps.clone(),
            provider_hints: self.provider_hints.clone(),
            ..SessionSummary::default()
        }
    }
}

fn base_event(value: &Value, role: Role) -> Option<NormalizedEvent> {
    let created = value.pointer("/time/created")?.as_i64()?;
    let mut event = NormalizedEvent::new(role);
    event.ts_ms = Some(created);
    Some(event)
}

fn model_ref(value: Option<&Value>) -> Option<(&str, &str)> {
    let value = value?;
    let model = value.get("id")?.as_str()?;
    let provider = value.get("providerID")?.as_str()?;
    [model, provider]
        .iter()
        .all(|value| !value.is_empty() && value.len() <= EVIDENCE_STRING_CAP)
        .then_some((model, provider))
}

fn capture_injected_skill(
    value: &Value,
    origin: JevNativeSkillOrigin,
    message: &SnapshotMessage,
    message_id: &str,
    output: &mut CapturedContent,
) -> bool {
    let (id_key, prefix) = match origin {
        JevNativeSkillOrigin::PromptAttachment { index } => ("id", format!("/skills/{index}")),
        JevNativeSkillOrigin::SkillMessage => ("skill", String::new()),
        _ => return false,
    };
    let identity = |key: &str| {
        value.get(key).and_then(Value::as_str).filter(|value| {
            !value.is_empty() && value.len() <= 256 && !value.chars().any(char::is_control)
        })
    };
    let (Some(id), Some(name)) = (
        identity(id_key).filter(|id| tools::valid_skill_name(id)),
        identity("name"),
    ) else {
        return false;
    };
    let text = value
        .get("text")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let index = output.parts.len();
    let Ok(part_index) = u32::try_from(index) else {
        return false;
    };
    output.push(ContentKind::UserText, text, None, None);
    let Some(part) = output.parts.get_mut(index) else {
        return false;
    };
    part.authority = ContentAuthority::Unknown;
    let body = if part.truncated {
        JevNativeSkillBody::Truncated
    } else if part.text.is_empty() {
        JevNativeSkillBody::Unavailable
    } else {
        JevNativeSkillBody::Recorded
    };
    for (key, length) in [(id_key, id.len()), ("name", name.len())] {
        part.metadata.bindings.push(JevNativeFieldRange {
            native_record_id: Some(message_id.to_owned()),
            field: JevInputField::UserMessage,
            container: JevNativeFieldContainer::Record,
            pointer: format!("{prefix}/{key}"),
            start: 0,
            end: length,
        });
    }
    if !part.text.is_empty() {
        part.metadata.bindings.push(JevNativeFieldRange {
            native_record_id: Some(message_id.to_owned()),
            field: JevInputField::UserMessage,
            container: JevNativeFieldContainer::Record,
            pointer: format!("{prefix}/text"),
            start: 0,
            end: part.text.len(),
        });
    }
    part.metadata.recorded_skill_result = Some(JevRecordedSkillResult::opencode_v2(
        part,
        if message.session.transfer {
            SourceFormat::OpenCodeV2SessionExport
        } else {
            SourceFormat::OpenCodeSqliteV2
        },
        &message.session_id,
        message_id,
        part_index,
        JevNativeSkillProof {
            producer: JevNativeSkillProducer::OpenCodeV2Skill843a6c51,
            origin,
            id: id.to_owned(),
            display_name: Some(name.to_owned()),
            body,
        },
    ));
    output.incomplete |= body != JevNativeSkillBody::Recorded;
    value.get("text").is_none_or(Value::is_string)
}

#[derive(Default)]
struct CapturedContent {
    parts: Vec<ContentPart>,
    retained_bytes: usize,
    oversized: bool,
    omitted_non_text: bool,
    incomplete: bool,
}

impl CapturedContent {
    fn push(
        &mut self,
        kind: ContentKind,
        text: &str,
        tool_name: Option<&str>,
        call_id: Option<&str>,
    ) {
        if text.is_empty()
            && !matches!(
                kind,
                ContentKind::ToolInput | ContentKind::ToolResult | ContentKind::UserText
            )
        {
            return;
        }
        if self.parts.len() >= MAX_USER_MESSAGE_PARTS {
            self.oversized = true;
            return;
        }
        let remaining = MAX_CONTENT_PART_BYTES.saturating_sub(self.retained_bytes);
        if remaining == 0 && !text.is_empty() {
            self.oversized = true;
            return;
        }
        let limit = text.len().min(MAX_CONTENT_PART_BYTES).min(remaining);
        let mut end = limit;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        if end < text.len() {
            self.oversized = true;
        }
        let mut part = ContentPart::new(kind, &text[..end]);
        part.truncated = end < text.len();
        if let Some(tool_name) = tool_name {
            part = part.with_tool_identity(Some(tool_name.to_owned()), call_id.map(str::to_owned));
        }
        self.retained_bytes = self.retained_bytes.saturating_add(part.text.len());
        self.parts.push(part);
    }
}

fn capture_assistant_text(value: &Value, output: &mut CapturedContent) -> bool {
    let Some(text) = value.get("text").and_then(Value::as_str) else {
        return false;
    };
    output.push(ContentKind::AssistantText, text, None, None);
    true
}
