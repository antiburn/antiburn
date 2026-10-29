//! Public message boundaries from the pinned V2 execution events.

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use serde_json::Value;

use super::{Gap, JoinedToolResult, MAX_RETAINED_CALLS, MAX_STRING_BYTES, OpenCodeV2EventJoiner};
use crate::analysis::{
    EVIDENCE_STRING_CAP, NativeActivityObservation, NativeCycleKey, NativeEventContext,
    NativeEvidenceEvent, NativeEvidenceFact, NativeMessageObservation, NativeRequestAttribution,
    NativeSessionSelectionObservation, NativeToolFacts, NativeToolInput, PublicMessageObservation,
    PublicMessagePayload, ResponseCycleObservation, ResponseCycleStatus, SourceFormat,
    ToolResultChange, ToolResultCompleteness, ToolResultPosition, UserMessage, UserMessageChange,
    UserMessageCompleteness, UserMessageObservation, UserMessageOrigin, UserMessagePart,
    UserMessagePosition, UserMessageScope,
};

const MAX_PUBLIC_BYTES: usize = 256 * 1024;
const MAX_CYCLE_MESSAGES: usize = 4096;
const MAX_CYCLE_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug, Default)]
pub(super) struct PublicState {
    baseline: Option<u64>,
    steps: BTreeMap<String, PublicStep>,
    calls: BTreeMap<(String, String), RetainedMessage>,
    cycle: Option<Cycle>,
    unavailable: bool,
    output_omitted: bool,
    request_history_incomplete: bool,
}

#[derive(Debug)]
struct PublicStep {
    requester: NativeRequestAttribution,
    context: NativeEventContext,
    prefix: Arc<UserPrefix>,
    text: BTreeMap<u32, Option<String>>,
    settled: bool,
    invalid: bool,
    published: Option<RetainedMessage>,
}

#[derive(Debug)]
struct UserPrefix {
    messages: Vec<UserMessage>,
    bytes: usize,
}

impl UserPrefix {
    fn new(messages: Vec<UserMessage>) -> Self {
        let bytes = encoded_bytes(&messages);
        Self { messages, bytes }
    }
}

// Retained contexts keep user messages here. Output observations own their messages.
#[derive(Debug, Clone)]
struct RetainedMessage {
    message: PublicMessageObservation,
    prefix: Arc<UserPrefix>,
}

impl RetainedMessage {
    fn materialize(&self) -> PublicMessageObservation {
        let mut message = self.message.clone();
        if let Some(context) = &mut message.context {
            context.user_messages = self.prefix.messages.clone();
        }
        message
    }

    fn retained_bytes(&self, prefixes: &mut BTreeSet<*const UserPrefix>) -> usize {
        encoded_bytes(&self.message) + prefix_bytes(&self.prefix, prefixes)
    }
}

#[derive(Debug)]
struct Cycle {
    event_id: String,
    sequence: u64,
    started_at_ms: Option<i64>,
    messages: Vec<NativeMessageObservation>,
    requests: Vec<NativeRequestAttribution>,
    steps: BTreeSet<String>,
    owned_steps: BTreeSet<String>,
    calls: BTreeSet<(String, String)>,
    delivered_input: bool,
    invalid: bool,
    bytes: usize,
}

impl Cycle {
    fn retain_message(&mut self, message: &NativeMessageObservation) {
        if self.invalid {
            return;
        }
        let prior = match message {
            NativeMessageObservation::Public(current) => self.messages.iter().position(|message| {
                matches!(message, NativeMessageObservation::Public(previous)
                        if previous.source_format == current.source_format
                            && previous.session_id == current.session_id
                            && previous.record_id == current.record_id
                            && previous.part_id == current.part_id
                            && previous.call_id == current.call_id)
            }),
            _ => None,
        };
        if let Some(index) = prior {
            self.bytes = self
                .bytes
                .saturating_sub(encoded_bytes(&self.messages[index]));
        }
        self.bytes = self.bytes.saturating_add(encoded_bytes(message));
        if (prior.is_none() && self.messages.len() >= MAX_CYCLE_MESSAGES)
            || self.bytes > MAX_CYCLE_BYTES
        {
            self.invalid = true;
            self.messages.clear();
            self.bytes = 0;
        } else if let Some(index) = prior {
            self.messages[index] = message.clone();
        } else {
            self.messages.push(message.clone());
        }
    }
}

impl PublicState {
    pub(super) fn set_baseline(&mut self, sequence: u64) {
        if self.baseline.is_some() {
            self.unavailable = true;
        }
        self.baseline = Some(sequence);
        self.cycle = None;
    }

    pub(super) fn clear(&mut self) {
        self.steps.clear();
        self.calls.clear();
        self.cycle = None;
        self.unavailable = true;
        self.request_history_incomplete = true;
    }

    pub(super) fn invalidate_cycle(&mut self) {
        if let Some(cycle) = &mut self.cycle {
            cycle.invalid = true;
        }
    }

    pub(super) fn has_pending_cycle(&self) -> bool {
        self.cycle.is_some()
    }

    fn evict_settled_step(&mut self) -> bool {
        let oldest = self
            .steps
            .iter()
            .filter(|(_, step)| step.settled)
            .min_by_key(|(_, step)| match step.context.first_eligible_position {
                ToolResultPosition::OpenCodeV2Event { sequence, .. } => sequence,
                _ => 0,
            })
            .map(|(id, _)| id.clone());
        oldest.is_some_and(|id| self.steps.remove(&id).is_some())
    }

    pub(super) fn take_omission(&mut self) -> bool {
        std::mem::take(&mut self.output_omitted)
    }

    pub(super) fn retained_bytes(&self) -> usize {
        let mut prefixes = BTreeSet::new();
        let steps = self
            .steps
            .iter()
            .map(|(id, step)| {
                id.len()
                    + encoded_bytes(&step.context)
                    + prefix_bytes(&step.prefix, &mut prefixes)
                    + step.requester.retained_bytes()
                    + step
                        .text
                        .values()
                        .map(|text| 32 + text.as_ref().map_or(0, String::len))
                        .sum::<usize>()
                    + step
                        .published
                        .as_ref()
                        .map_or(0, |message| message.retained_bytes(&mut prefixes))
            })
            .sum::<usize>();
        steps
            + self
                .calls
                .iter()
                .map(|((message, call), value)| {
                    message.len() + call.len() + value.retained_bytes(&mut prefixes)
                })
                .sum::<usize>()
            + self.cycle.as_ref().map_or(0, |cycle| {
                cycle.bytes
                    + cycle.event_id.len()
                    + cycle
                        .requests
                        .iter()
                        .map(|request| request.retained_bytes() + 32)
                        .sum::<usize>()
                    + cycle.steps.iter().map(String::len).sum::<usize>()
                    + cycle.owned_steps.iter().map(String::len).sum::<usize>()
                    + cycle
                        .calls
                        .iter()
                        .map(|(message, call)| message.len() + call.len())
                        .sum::<usize>()
            })
    }

    pub(super) fn observe(
        &mut self,
        joiner: &OpenCodeV2EventJoiner,
        event: &Value,
        sequence: u64,
        session: &str,
        result: Option<&JoinedToolResult>,
    ) -> Vec<NativeEvidenceEvent> {
        let kind = event["type"].as_str().unwrap_or("");
        let data = &event["data"];
        let live = self.baseline.is_none_or(|baseline| sequence > baseline);
        self.request_history_incomplete |= joiner.request_history_gap || !joiner.gaps.is_empty();
        let safe = session.len() <= EVIDENCE_STRING_CAP
            && !joiner
                .gaps
                .iter()
                .any(|gap| !matches!(gap, Gap::MissingJoin));
        if !safe {
            self.invalidate_cycle();
        }
        if step_owned_work(kind)
            && self.cycle.as_ref().is_some_and(|cycle| {
                identity(&data["assistantMessageID"])
                    .is_none_or(|id| !cycle.owned_steps.contains(id) || !cycle.steps.contains(id))
            })
        {
            self.invalidate_cycle();
        }
        let mut messages = Vec::new();
        let mut facts = Vec::new();
        match kind {
            "session.model.selected" => {
                let selection = attribution(&data["model"]);
                if safe && selection.model_key().is_some() {
                    facts.push(NativeEvidenceFact::SessionSelection(
                        NativeSessionSelectionObservation {
                            source_format: SourceFormat::OpenCodeSqliteV2,
                            session_id: session.to_owned(),
                            position: position(sequence),
                            selection,
                        },
                    ));
                }
            }
            "session.execution.started" => {
                if self.cycle.is_some() {
                    self.invalidate_cycle();
                    self.unavailable = true;
                } else if live
                    && safe
                    && self.baseline.is_some()
                    && !self.unavailable
                    && identity(&event["id"]).is_some()
                {
                    self.cycle = Some(Cycle {
                        event_id: event["id"].as_str().unwrap_or("").to_owned(),
                        sequence,
                        started_at_ms: native_time(event),
                        messages: Vec::new(),
                        requests: Vec::new(),
                        steps: BTreeSet::new(),
                        owned_steps: BTreeSet::new(),
                        calls: BTreeSet::new(),
                        delivered_input: false,
                        invalid: false,
                        bytes: 0,
                    });
                }
            }
            "session.inbox.delivered" => {
                let request = joiner
                    .users
                    .as_ref()
                    .map(|history| &history.request)
                    .filter(|request| request.sequence == sequence);
                if let Some(request) = request {
                    if let Some(message) = user(
                        session,
                        &request.message_id,
                        request.sequence,
                        request.text.as_deref(),
                        request.complete,
                    ) {
                        let mut prefix = users(joiner, session);
                        prefix.retain(|user| matches!(user.position, UserMessagePosition::OpenCodeV2Event { sequence: value } if value <= sequence));
                        messages.push(NativeMessageObservation::User(UserMessageObservation {
                            message,
                            revision: sequence,
                            context: NativeEventContext {
                                first_eligible_position: position(sequence),
                                main_session: safe,
                                complete_through_event: !joiner.request_history_gap
                                    && request.complete,
                                user_messages: prefix,
                            },
                        }));
                        if let Some(cycle) = &mut self.cycle {
                            cycle.delivered_input = true;
                            cycle.invalid |= !request.complete;
                        }
                    } else {
                        self.invalidate_cycle();
                    }
                } else {
                    // Non-user delivery is control work. Missing input must not become request text.
                    self.invalidate_cycle();
                }
            }
            "session.step.started" => {
                if let Some(id) = identity(&data["assistantMessageID"]) {
                    let context = joiner.steps.get(id).map(|step| &step.context);
                    let requester = context.map_or_else(
                        || attribution(&data["model"]),
                        |context| context.requester.clone(),
                    );
                    let prefix = context.map_or_else(Vec::new, |context| {
                        context
                            .user_messages()
                            .iter()
                            .filter_map(|message| {
                                user(
                                    session,
                                    &message.message_id,
                                    message.sequence,
                                    message.text.as_deref(),
                                    message.complete,
                                )
                            })
                            .collect()
                    });
                    let prior = self.steps.remove(id);
                    let mut published = prior.and_then(|step| step.published);
                    if let Some(previous) = &mut published {
                        previous.message.eligible = false;
                        previous.message.position = position(sequence);
                        previous.message.revision = sequence;
                        messages.push(NativeMessageObservation::Public(previous.materialize()));
                    }
                    if self.steps.len() >= MAX_RETAINED_CALLS {
                        self.evict_settled_step();
                    }
                    if self.steps.len() >= MAX_RETAINED_CALLS {
                        self.unavailable = true;
                        self.invalidate_cycle();
                    } else {
                        self.steps.insert(
                            id.to_owned(),
                            PublicStep {
                                requester: requester.clone(),
                                text: BTreeMap::new(),
                                settled: false,
                                invalid: false,
                                published,
                                prefix: Arc::new(UserPrefix::new(prefix)),
                                context: NativeEventContext {
                                    first_eligible_position: position(sequence),
                                    main_session: safe,
                                    complete_through_event: context
                                        .is_some_and(|context| context.gaps.is_empty()),
                                    user_messages: Vec::new(),
                                },
                            },
                        );
                    }
                    if let Some(cycle) = &mut self.cycle {
                        cycle.invalid |= !cycle.delivered_input || cycle.owned_steps.contains(id);
                        if cycle.requests.len() >= MAX_RETAINED_CALLS {
                            cycle.invalid = true;
                        } else {
                            cycle.requests.push(requester);
                        }
                        if cycle.owned_steps.len() >= MAX_RETAINED_CALLS {
                            cycle.invalid = true;
                        } else {
                            cycle.steps.insert(id.to_owned());
                            cycle.owned_steps.insert(id.to_owned());
                        }
                    }
                } else {
                    self.invalidate_cycle();
                }
            }
            "session.text.started" | "session.text.ended" => {
                let slot = data["ordinal"]
                    .as_u64()
                    .and_then(|value| u32::try_from(value).ok());
                let step =
                    identity(&data["assistantMessageID"]).and_then(|id| self.steps.get_mut(id));
                if let (Some(step), Some(slot)) = (step, slot) {
                    if kind == "session.text.started" {
                        if step.text.len() >= MAX_RETAINED_CALLS
                            || step.text.insert(slot, None).is_some()
                            || step.settled
                        {
                            step.invalid = true;
                        }
                    } else if let Some(text) = data["text"]
                        .as_str()
                        .filter(|text| text.len() <= MAX_STRING_BYTES)
                    {
                        if let Some(value) = step.text.get_mut(&slot) {
                            if step.settled && value.as_deref() != Some(text) {
                                step.invalid = true;
                                if let Some(previous) = &mut step.published {
                                    previous.message.eligible = false;
                                    previous.message.position = position(sequence);
                                    previous.message.revision = sequence;
                                    messages.push(NativeMessageObservation::Public(
                                        previous.materialize(),
                                    ));
                                }
                            }
                            *value = Some(text.to_owned());
                        } else {
                            step.invalid = true;
                        }
                    } else {
                        step.invalid = true;
                    }
                } else {
                    self.invalidate_cycle();
                }
            }
            "session.tool.called" => {
                if let (Some(message), Some(call_id)) =
                    (identity(&data["assistantMessageID"]), identity(&data["id"]))
                {
                    let key = (message.to_owned(), call_id.to_owned());
                    if let (Some(step), Some(call)) =
                        (self.steps.get(message), joiner.calls.get(&key))
                    {
                        if let (Some(name), Some(input)) = (&call.name, &call.input) {
                            if name.len() <= EVIDENCE_STRING_CAP
                                && input.is_object()
                                && data["executed"].is_boolean()
                            {
                                let facts = NativeToolFacts::for_call(
                                    SourceFormat::OpenCodeSqliteV2,
                                    if call.provider_executed == Some(false) {
                                        name
                                    } else {
                                        ""
                                    },
                                    Some(input),
                                    None,
                                    step.requester.clone(),
                                );
                                let mut observation = RetainedMessage {
                                    prefix: step.prefix.clone(),
                                    message: PublicMessageObservation {
                                        source_format: SourceFormat::OpenCodeSqliteV2,
                                        session_id: session.to_owned(),
                                        record_id: message.to_owned(),
                                        part_id: None,
                                        call_id: Some(call_id.to_owned()),
                                        position: position(sequence),
                                        revision: sequence,
                                        change: ToolResultChange::Update,
                                        eligible: safe,
                                        completeness: completeness(session, safe),
                                        context: Some(step.context.clone()),
                                        payload: PublicMessagePayload::ToolCall {
                                            tool_name: name.clone(),
                                            input: Some(NativeToolInput::Json(input.clone())),
                                            facts: Some(facts),
                                            provider_executed: call.provider_executed,
                                        },
                                    },
                                };
                                let unchanged = self.calls.get(&key).is_some_and(|previous| {
                                    previous.message.payload == observation.message.payload
                                });
                                if let Some(previous) = self.calls.get(&key) {
                                    observation.message.context = previous.message.context.clone();
                                    observation.prefix = previous.prefix.clone();
                                } else if let Some(context) = &mut observation.message.context {
                                    context.first_eligible_position = position(sequence);
                                }
                                if !unchanged
                                    && (self.calls.len() < MAX_RETAINED_CALLS
                                        || self.calls.contains_key(&key))
                                {
                                    self.calls.insert(key.clone(), observation.clone());
                                    messages.push(NativeMessageObservation::Public(
                                        observation.materialize(),
                                    ));
                                    if let Some(cycle) = &mut self.cycle {
                                        cycle.invalid |= !cycle.owned_steps.contains(message);
                                        cycle.calls.insert(key);
                                    }
                                } else if !unchanged {
                                    self.invalidate_cycle();
                                }
                            } else {
                                self.invalidate_cycle();
                            }
                        } else {
                            self.invalidate_cycle();
                        }
                    } else {
                        self.invalidate_cycle();
                    }
                } else {
                    self.invalidate_cycle();
                }
            }
            "session.step.ended" | "session.step.failed" => {
                if let Some(id) = identity(&data["assistantMessageID"]) {
                    if let Some(step) = self.steps.get_mut(id) {
                        step.settled = true;
                        let success = kind == "session.step.ended"
                            && matches!(data["finish"].as_str(), Some("stop" | "tool-calls"));
                        let final_text = !step.invalid && step.text.values().all(Option::is_some);
                        if success && final_text && !step.text.is_empty() {
                            let parts = step
                                .text
                                .iter()
                                .filter_map(|(ordinal, text)| {
                                    Some(UserMessagePart {
                                        item_index: *ordinal,
                                        native_part_id: None,
                                        text: text.clone()?,
                                    })
                                })
                                .collect();
                            let mut context = step
                                .published
                                .as_ref()
                                .and_then(|previous| previous.message.context.clone())
                                .unwrap_or_else(|| step.context.clone());
                            if step.published.is_none() {
                                context.first_eligible_position = position(sequence);
                            }
                            let observation = RetainedMessage {
                                prefix: step.published.as_ref().map_or_else(
                                    || step.prefix.clone(),
                                    |previous| previous.prefix.clone(),
                                ),
                                message: PublicMessageObservation {
                                    source_format: SourceFormat::OpenCodeSqliteV2,
                                    session_id: session.to_owned(),
                                    record_id: id.to_owned(),
                                    part_id: None,
                                    call_id: None,
                                    position: position(sequence),
                                    revision: sequence,
                                    change: ToolResultChange::Update,
                                    eligible: safe,
                                    completeness: completeness(session, safe),
                                    context: Some(context),
                                    payload: PublicMessagePayload::AssistantText {
                                        parts,
                                        generator: step.requester.clone(),
                                    },
                                },
                            };
                            step.published = Some(observation.clone());
                            messages
                                .push(NativeMessageObservation::Public(observation.materialize()));
                        } else if let Some(previous) = &mut step.published {
                            previous.message.position = position(sequence);
                            previous.message.revision = sequence;
                            previous.message.eligible = false;
                            messages.push(NativeMessageObservation::Public(previous.materialize()));
                        }
                        if let Some(cycle) = &mut self.cycle {
                            cycle.invalid |= !cycle.steps.remove(id);
                            cycle.invalid |= step.invalid
                                || (!step.text.is_empty() && (!success || !final_text));
                        }
                    } else {
                        self.invalidate_cycle();
                    }
                } else {
                    self.invalidate_cycle();
                }
            }
            "session.compaction.started" | "session.compaction.ended" | "session.synthetic" => {
                self.invalidate_cycle()
            }
            _ => {}
        }
        if let Some(result) = result {
            let mut observation = result.clone().into_observation(false, true);
            observation.revision = sequence;
            let key = (result.assistant_message_id.clone(), result.call_id.clone());
            if let Some(call) = self.calls.get_mut(&key)
                && let PublicMessagePayload::ToolCall {
                    facts,
                    provider_executed,
                    ..
                } = &mut call.message.payload
                && *provider_executed != observation.provider_executed
            {
                *provider_executed = observation.provider_executed;
                *facts = observation.facts.clone();
                call.message.position = position(sequence);
                call.message.revision = sequence;
                call.message.eligible &= safe;
                call.message.completeness = completeness(session, safe);
                messages.push(NativeMessageObservation::Public(call.materialize()));
            }
            self.calls.remove(&key);
            if let Some(cycle) = &mut self.cycle {
                cycle.invalid |= observation.content.is_none() || observation.arguments.is_none();
                cycle.invalid |= !cycle.calls.remove(&key)
                    || result
                        .gaps
                        .iter()
                        .any(|gap| !matches!(gap, Gap::MissingJoin));
            }
            messages.push(NativeMessageObservation::ToolResult(observation));
        }
        for message in messages {
            if let Some(cycle) = &mut self.cycle {
                cycle.retain_message(&message);
            }
            facts.push(NativeEvidenceFact::Message(message));
        }
        if matches!(
            kind,
            "session.execution.succeeded"
                | "session.execution.failed"
                | "session.execution.interrupted"
        ) {
            if kind == "session.execution.interrupted"
                && !matches!(data["reason"].as_str(), Some("user" | "inactivity"))
            {
                self.unavailable = true;
            }
            if let Some(cycle) = self.cycle.take() {
                let status = match (kind, data["reason"].as_str()) {
                    ("session.execution.succeeded", _) => Some(ResponseCycleStatus::Completed),
                    ("session.execution.failed", _) if data["error"]["type"].is_string() => {
                        Some(ResponseCycleStatus::Error)
                    }
                    ("session.execution.interrupted", Some("user" | "inactivity")) => {
                        Some(ResponseCycleStatus::Cancelled)
                    }
                    _ => None,
                };
                let unfinished = !cycle.steps.is_empty() || !cycle.calls.is_empty();
                if safe
                    && !cycle.invalid
                    && !unfinished
                    && cycle.delivered_input
                    && !cycle.requests.is_empty()
                    && let Some(status) = status
                {
                    let prefix = users(joiner, session);
                    let history_complete = !self.request_history_incomplete
                        && joiner
                            .users
                            .as_ref()
                            .is_some_and(|history| history.len == prefix.len())
                        && prefix.iter().all(|message| {
                            message.completeness == UserMessageCompleteness::Complete
                        });
                    facts.push(NativeEvidenceFact::ResponseCycle(
                        ResponseCycleObservation {
                            source_format: SourceFormat::OpenCodeSqliteV2,
                            session_id: session.to_owned(),
                            key: NativeCycleKey::OpenCodeV2Execution {
                                start_event_id: cycle.event_id,
                                start_sequence: cycle.sequence,
                            },
                            start_position: position(cycle.sequence),
                            position: position(sequence),
                            revision: sequence,
                            change: ToolResultChange::Update,
                            status,
                            native_reason: data["reason"].as_str().map(str::to_owned),
                            native_error_code: data["error"]["type"]
                                .as_str()
                                .filter(|code| code.len() <= EVIDENCE_STRING_CAP)
                                .map(str::to_owned),
                            started_at_ms: cycle.started_at_ms,
                            ended_at_ms: native_time(event),
                            requester: uniform(&cycle.requests),
                            context: NativeEventContext {
                                first_eligible_position: position(sequence),
                                main_session: true,
                                complete_through_event: history_complete,
                                user_messages: prefix,
                            },
                            messages: cycle.messages,
                        },
                    ));
                }
            }
            self.steps.retain(|_, step| step.settled);
            self.calls.clear();
        }
        if self.retained_bytes() > MAX_PUBLIC_BYTES {
            if let Some(cycle) = &mut self.cycle {
                cycle.invalid = true;
                cycle.messages.clear();
                cycle.bytes = 0;
            }
            while self.retained_bytes() > MAX_PUBLIC_BYTES && self.evict_settled_step() {}
            if self.retained_bytes() > MAX_PUBLIC_BYTES {
                self.clear();
                self.output_omitted = true;
            }
        }
        self.finish(event, session, sequence, live, safe, facts)
    }

    fn finish(
        &mut self,
        event: &Value,
        session: &str,
        sequence: u64,
        live: bool,
        safe: bool,
        mut facts: Vec<NativeEvidenceFact>,
    ) -> Vec<NativeEvidenceEvent> {
        if safe && activity_kind(event["type"].as_str().unwrap_or("")) {
            facts.push(NativeEvidenceFact::Activity(NativeActivityObservation {
                source_format: SourceFormat::OpenCodeSqliteV2,
                session_id: session.to_owned(),
                position: position(sequence),
                token: event["id"].as_str().unwrap_or("").to_owned(),
                native_at_ms: native_time(event),
            }));
        }
        if encoded_bytes(&facts) > MAX_PUBLIC_BYTES {
            facts.retain(|fact| !matches!(fact, NativeEvidenceFact::ResponseCycle(_)));
        }
        let events: Vec<_> = facts
            .into_iter()
            .map(|fact| {
                let first = match &fact {
                    NativeEvidenceFact::Message(NativeMessageObservation::User(user)) => user.context.first_eligible_position,
                    NativeEvidenceFact::Message(NativeMessageObservation::Public(public)) => public.context.as_ref().map_or(public.position, |context| context.first_eligible_position),
                    _ => position(sequence),
                };
                let after_baseline = self.baseline.is_none_or(|baseline| matches!(first, ToolResultPosition::OpenCodeV2Event { sequence, .. } if sequence > baseline));
                NativeEvidenceEvent { fact, live_eligible: live && safe && after_baseline }
            })
            .collect();
        if encoded_bytes(&events) > MAX_PUBLIC_BYTES {
            self.output_omitted = true;
            self.invalidate_cycle();
            Vec::new()
        } else {
            events
        }
    }
}

fn encoded_bytes(value: &impl serde::Serialize) -> usize {
    serde_json::to_vec(value).map_or(usize::MAX, |bytes| bytes.len())
}

fn prefix_bytes(prefix: &Arc<UserPrefix>, prefixes: &mut BTreeSet<*const UserPrefix>) -> usize {
    if prefixes.insert(Arc::as_ptr(prefix)) {
        prefix.bytes
    } else {
        0
    }
}

fn position(sequence: u64) -> ToolResultPosition {
    ToolResultPosition::OpenCodeV2Event {
        sequence,
        item_index: 0,
    }
}

fn completeness(session: &str, ordered: bool) -> ToolResultCompleteness {
    if session.len() > EVIDENCE_STRING_CAP {
        ToolResultCompleteness::UnsafeIdentity
    } else if !ordered {
        ToolResultCompleteness::UnsafeOrder
    } else {
        ToolResultCompleteness::Complete
    }
}

fn identity(value: &Value) -> Option<&str> {
    value
        .as_str()
        .filter(|id| !id.is_empty() && id.len() <= EVIDENCE_STRING_CAP)
}

fn native_time(event: &Value) -> Option<i64> {
    event["created"].as_i64()
}

fn user(
    session: &str,
    id: &str,
    sequence: u64,
    text: Option<&str>,
    complete: bool,
) -> Option<UserMessage> {
    let message = UserMessage {
        source_format: SourceFormat::OpenCodeSqliteV2,
        session_id: session.to_owned(),
        message_id: id.to_owned(),
        position: UserMessagePosition::OpenCodeV2Event { sequence },
        origin: UserMessageOrigin::OpenCodeUserText,
        scope: UserMessageScope::MainSession,
        change: UserMessageChange::Present,
        completeness: if complete {
            UserMessageCompleteness::Complete
        } else {
            UserMessageCompleteness::Incomplete
        },
        parts: vec![UserMessagePart {
            item_index: 0,
            native_part_id: None,
            text: text?.to_owned(),
        }],
        proof_revision: 1,
    };
    message.is_bounded().then_some(message)
}

fn users(joiner: &OpenCodeV2EventJoiner, session: &str) -> Vec<UserMessage> {
    super::StepContext {
        users: joiner.users.clone(),
        ..super::StepContext::default()
    }
    .user_messages()
    .iter()
    .filter_map(|request| {
        user(
            session,
            &request.message_id,
            request.sequence,
            request.text.as_deref(),
            request.complete,
        )
    })
    .collect()
}

pub(super) fn attribution(model: &Value) -> NativeRequestAttribution {
    let mut requester =
        NativeRequestAttribution::observed(model["providerID"].as_str(), model["id"].as_str());
    // Pin 843a6c514e606b436a7fe713422ace0a93ad3d61 names these IDs in
    // packages/core/src/models-dev/snapshot.txt and models-dev.ts expand them.
    requester.fast_mode = match (requester.provider.as_deref(), requester.model.as_deref()) {
        (
            Some("openai"),
            Some(
                "gpt-5.5-fast"
                | "gpt-5.6-luna-fast"
                | "gpt-5.6-sol-fast"
                | "gpt-5.6-terra-fast"
                | "gpt-6-astra-fast"
                | "gpt-6-astra-ultrafast"
                | "gpt-6-luna-fast"
                | "gpt-6-sol-fast"
                | "gpt-6.1-sol-fast"
                | "gpt-6.1-sol-ultrafast",
            ),
        ) => Some(true),
        _ => None,
    };
    requester
}

fn uniform(requests: &[NativeRequestAttribution]) -> NativeRequestAttribution {
    let Some(first) = requests.first() else {
        return NativeRequestAttribution::default();
    };
    let model_uniform = first.model_key().is_some()
        && requests
            .iter()
            .all(|request| request.provider == first.provider && request.model == first.model);
    NativeRequestAttribution {
        provider: model_uniform.then(|| first.provider.clone()).flatten(),
        model: model_uniform.then(|| first.model.clone()).flatten(),
        fast_mode: first.fast_mode.filter(|mode| {
            requests
                .iter()
                .all(|request| request.fast_mode == Some(*mode))
        }),
    }
}

fn activity_kind(kind: &str) -> bool {
    matches!(
        kind,
        "session.inbox.enqueued"
            | "session.inbox.delivered"
            | "session.inbox.cancelled"
            | "session.inbox.delivery.changed"
            | "session.execution.started"
            | "session.execution.succeeded"
            | "session.execution.failed"
            | "session.execution.interrupted"
            | "session.step.started"
            | "session.step.streamed"
            | "session.step.ended"
            | "session.step.failed"
            | "session.text.started"
            | "session.text.ended"
            | "session.reasoning.started"
            | "session.reasoning.ended"
            | "session.tool.input.started"
            | "session.tool.input.ended"
            | "session.tool.called"
            | "session.tool.success"
            | "session.tool.failed"
            | "session.shell.started"
            | "session.shell.ended"
            | "session.retry.scheduled"
            | "session.compaction.started"
            | "session.compaction.ended"
            | "session.compaction.failed"
    )
}

fn step_owned_work(kind: &str) -> bool {
    matches!(
        kind,
        "session.step.streamed"
            | "session.text.started"
            | "session.text.delta"
            | "session.text.ended"
            | "session.reasoning.started"
            | "session.reasoning.delta"
            | "session.reasoning.ended"
            | "session.tool.input.started"
            | "session.tool.input.delta"
            | "session.tool.input.ended"
            | "session.tool.called"
            | "session.tool.progress"
            | "session.tool.success"
            | "session.tool.failed"
    )
}
