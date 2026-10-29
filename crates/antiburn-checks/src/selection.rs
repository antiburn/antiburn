//! Per-branch event filtering and bounded candidate selection.

use std::collections::{BTreeMap, VecDeque};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::compiler::{
    CompiledCheck, EventFilters, RollingWindow, ToolCategory, ToolResultStatus, TriggerFamily,
};
use crate::definition_lifecycle::EffectiveActivation;
use crate::domain::{
    BranchId, EvidenceDependency, EvidenceVersion, FrozenCandidate, FrozenContext, Message,
    NativeOrderKey, Observation, ObservationKind, ObservationPosition,
};
use crate::filters::{
    AttributionScope, FilterDiagnostic, evaluate_attribution_filters, evaluate_path_filters,
};
use crate::preparation::{ConditionOutcome, evaluate_trigger_condition};

/// Maximum number of selected messages retained for one branch.
pub const MAX_WINDOW_MESSAGES: usize = 40;
/// Maximum number of source identities retained for one branch and activation.
pub const MAX_SEEN_RESULTS_PER_BRANCH: usize = 4096;

/// The selector state for one activation, separated by branch.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SelectionState {
    /// Branch-local rolling windows required to resume selection after restart.
    pub branches: BTreeMap<BranchId, BranchSelection>,
    /// Latest revision and native order for each source identity seen by this activation.
    pub seen_results: BTreeMap<(BranchId, String), SeenResult>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BranchSelection {
    /// Selected messages in the order accepted by the source adapter.
    pub results: VecDeque<SelectedMessage>,
    /// Whether the latest source snapshot proved complete membership for this branch.
    pub coverage_complete: bool,
    /// Whether identity tracking reached its limit and now rejects unseen results.
    #[serde(default)]
    pub identity_limit_reached: bool,
    /// Latest model, mode, or path rejection, retained across restart.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_filter_diagnostic: Option<FilterDiagnostic>,
}

impl Default for BranchSelection {
    fn default() -> Self {
        Self {
            results: VecDeque::new(),
            coverage_complete: true,
            identity_limit_reached: false,
            last_filter_diagnostic: None,
        }
    }
}

/// Latest committed source version for one logical event identity.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SeenResult {
    /// Whether first eligibility or removal prevents another trigger for this identity.
    pub terminal_seen: bool,
    /// Highest producer revision observed for this identity.
    pub revision: u64,
    /// Latest exact evidence version, including same-revision payload changes.
    pub version: EvidenceVersion,
    /// Pending order can advance. Terminal corrections retain the first-terminal order.
    pub native_order: NativeOrderKey,
    /// Selection history survives window eviction and restart.
    #[serde(default, skip_serializing_if = "ResultMembership::is_unknown")]
    pub membership: ResultMembership,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResultMembership {
    /// An older checkpoint does not retain selection history.
    #[default]
    Unknown,
    /// The first terminal result precedes the activation boundary.
    Historical,
    /// The latest terminal value fails selection or lacks required context.
    Excluded,
    /// The result enters the rolling window.
    Selected,
    /// The result leaves the rolling window and cannot enter it again.
    Retired,
    /// A removal prevents an equal or older revision from restoring the result.
    Removed,
}

impl ResultMembership {
    fn is_unknown(&self) -> bool {
        *self == Self::Unknown
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SelectedMessage {
    /// Source version used to detect corrections.
    pub version: EvidenceVersion,
    /// Source-native order retained across corrections.
    pub native_order: NativeOrderKey,
    /// Immutable normalized message value.
    pub message: Message,
    /// Whether the source proved the selected result complete.
    pub complete: bool,
}

/// Result of applying one normalized observation to a selector.
#[derive(Clone, Debug, PartialEq)]
pub enum SelectionOutcome {
    /// The observation updated context or ordering but did not create a candidate.
    Updated,
    /// The result did not match the effective tool/status filters.
    Filtered,
    /// Captured model, mode, or path facts did not satisfy a filter.
    FilterRejected(FilterDiagnostic),
    /// The pre-selection condition rejected the trigger before window counting.
    ConditionFiltered(ConditionOutcome),
    /// Matching evidence is incomplete and cannot be assessed.
    Incomplete,
    /// A correction makes retained window membership or order uncertain.
    WindowInvalidated,
    /// A complete candidate is frozen at this trigger position.
    Candidate(Box<FrozenCandidate>),
}

impl SelectionState {
    /// Applies one event and freezes a candidate when its selection rules pass.
    /// Historical evidence cannot enter a fresh window. Published events use
    /// `apply_published` to retain progress, corrections, and removals.
    pub fn apply(
        &mut self,
        activation: &EffectiveActivation,
        observation: &Observation,
    ) -> SelectionOutcome {
        self.apply_with_inventories(activation, observation, &mut || {
            Ok::<_, std::convert::Infallible>(None)
        })
        .unwrap_or_else(|never| match never {})
    }

    fn apply_with_inventories<E>(
        &mut self,
        activation: &EffectiveActivation,
        observation: &Observation,
        capture: &mut impl FnMut() -> Result<Option<crate::context::CurrentInventories>, E>,
    ) -> Result<SelectionOutcome, E> {
        if matches!(observation.kind, ObservationKind::Other) {
            return Ok(SelectionOutcome::Updated);
        }
        if is_historical(activation, observation) {
            return Ok(SelectionOutcome::Updated);
        }
        let matches = evaluate_event_filters(&activation.compiled, observation);
        if let Err(diagnostic) = matches {
            self.branches
                .entry(observation.branch.clone())
                .or_default()
                .last_filter_diagnostic = Some(diagnostic.clone());
            return Ok(SelectionOutcome::FilterRejected(diagnostic));
        }
        if matches == Ok(false) {
            return Ok(SelectionOutcome::Filtered);
        }
        self.branches
            .entry(observation.branch.clone())
            .or_default()
            .last_filter_diagnostic = None;

        let coverage_complete = self
            .branches
            .get(&observation.branch)
            .is_none_or(|branch| branch.coverage_complete);
        let Some(mut context) = trigger_context(observation, coverage_complete) else {
            return Ok(SelectionOutcome::Incomplete);
        };
        let demand = activation.compiled.current_inventory_demand();
        if demand.condition
            && let Some(inventories) = capture()?
        {
            inventories.merge_into(&mut context.value);
        }
        let condition = evaluate_trigger_condition(&activation.compiled, &context.value);
        if condition != ConditionOutcome::Matched {
            return Ok(SelectionOutcome::ConditionFiltered(condition));
        }

        let Some(message) = observation_message(observation) else {
            if demand.candidate
                && !demand.condition
                && let Some(inventories) = capture()?
            {
                inventories.merge_into(&mut context.value);
            }
            let mut dependencies = context.dependencies.clone();
            dependencies.sort();
            dependencies.dedup();
            let messages = Vec::new();
            let fingerprint = candidate_fingerprint(
                &activation.id,
                &observation.branch,
                observation.position,
                &messages,
                &context,
                &dependencies,
            );
            return Ok(SelectionOutcome::Candidate(Box::new(FrozenCandidate {
                activation: activation.id.clone(),
                branch: observation.branch.clone(),
                trigger_position: observation.position,
                messages,
                context,
                dependencies,
                fingerprint,
            })));
        };

        let branch = self.branches.entry(observation.branch.clone()).or_default();
        let config = activation.compiled.trigger.window.unwrap_or(RollingWindow {
            size: 1,
            minimum: 1,
        });
        let size = usize::from(config.size).min(MAX_WINDOW_MESSAGES);
        branch.results.push_back(SelectedMessage {
            version: observation.version.clone(),
            native_order: observation.native_order.clone(),
            message: message.clone(),
            complete: observation.complete,
        });
        while branch.results.len() > size {
            let retired = branch
                .results
                .pop_front()
                .expect("the window exceeds its limit");
            if let Some(seen) = self
                .seen_results
                .get_mut(&(observation.branch.clone(), retired.message.id))
            {
                seen.membership = ResultMembership::Retired;
            }
        }
        if branch.results.len() < usize::from(config.minimum) {
            return Ok(SelectionOutcome::Updated);
        }
        if demand.candidate
            && !demand.condition
            && let Some(inventories) = capture()?
        {
            inventories.merge_into(&mut context.value);
        }
        let mut dependencies = branch
            .results
            .iter()
            .map(|item| EvidenceDependency::Message {
                id: item.message.id.clone(),
                version: item.version.clone(),
            })
            .collect::<Vec<_>>();
        dependencies.extend(observation.context.dependencies.clone());
        dependencies.push(EvidenceDependency::MembershipThrough(observation.position));
        dependencies.sort();
        dependencies.dedup();
        let messages = branch
            .results
            .iter()
            .map(|item| item.message.clone())
            .collect::<Vec<_>>();
        let roots = context
            .value
            .as_object_mut()
            .expect("trigger context is an object");
        if activation.compiled.trigger.window.is_some() {
            roots.insert(
                "window".to_owned(),
                serde_json::json!({"messages": messages.iter().map(|message| &message.value).collect::<Vec<_>>()}),
            );
        }
        let fingerprint = candidate_fingerprint(
            &activation.id,
            &observation.branch,
            observation.position,
            &messages,
            &context,
            &dependencies,
        );
        Ok(SelectionOutcome::Candidate(Box::new(FrozenCandidate {
            activation: activation.id.clone(),
            branch: observation.branch.clone(),
            trigger_position: observation.position,
            messages,
            context,
            dependencies,
            fingerprint,
        })))
    }

    /// Applies one published event without treating a correction as a new trigger.
    pub fn apply_published(
        &mut self,
        activation: &EffectiveActivation,
        identity: &str,
        revision: u64,
        observation: &Observation,
    ) -> SelectionOutcome {
        self.apply_published_with_inventories(
            activation,
            identity,
            revision,
            observation,
            &mut || Ok::<_, std::convert::Infallible>(None),
        )
        .unwrap_or_else(|never| match never {})
    }

    /// Captures current inventories only when selection needs them.
    /// Discard this selection state if capture returns an error.
    pub fn apply_published_with_inventories<E>(
        &mut self,
        activation: &EffectiveActivation,
        identity: &str,
        revision: u64,
        observation: &Observation,
        capture: &mut impl FnMut() -> Result<Option<crate::context::CurrentInventories>, E>,
    ) -> Result<SelectionOutcome, E> {
        let key = (observation.branch.clone(), identity.to_owned());
        if !self.seen_results.contains_key(&key)
            && self
                .branches
                .get(&observation.branch)
                .is_some_and(|branch| branch.identity_limit_reached)
        {
            return Ok(SelectionOutcome::Updated);
        }
        if !self.accepts_update(
            &observation.branch,
            identity,
            revision,
            &observation.version,
        ) {
            return Ok(SelectionOutcome::Updated);
        }
        if !self.seen_results.contains_key(&key)
            && self.identity_count(&observation.branch) >= MAX_SEEN_RESULTS_PER_BRANCH
        {
            self.mark_identity_limit(&observation.branch);
            return Ok(SelectionOutcome::Updated);
        }
        let previous = self.seen_results.get(&key).cloned();
        let terminal = !matches!(observation.kind, ObservationKind::Other);
        self.seen_results.insert(
            key.clone(),
            SeenResult {
                terminal_seen: terminal || previous.as_ref().is_some_and(|seen| seen.terminal_seen),
                revision,
                version: observation.version.clone(),
                native_order: observation.native_order.clone(),
                membership: previous
                    .as_ref()
                    .map_or(ResultMembership::Unknown, |seen| match seen.membership {
                        ResultMembership::Removed => ResultMembership::Retired,
                        membership => membership,
                    }),
            },
        );
        if terminal && is_historical(activation, observation) {
            self.discard_result(&observation.branch, identity);
            self.set_membership(&key, ResultMembership::Historical);
            return Ok(SelectionOutcome::Updated);
        }
        if previous.as_ref().is_none_or(|seen| !seen.terminal_seen) {
            if !terminal {
                return Ok(SelectionOutcome::Updated);
            }
            let outcome = self.apply_with_inventories(activation, observation, capture)?;
            let membership = match outcome {
                SelectionOutcome::Candidate(_) | SelectionOutcome::Updated => {
                    ResultMembership::Selected
                }
                _ => ResultMembership::Excluded,
            };
            self.set_membership(&key, membership);
            return Ok(outcome);
        }
        let previous = previous.expect("a terminal result was recorded");
        if matches!(
            activation.compiled.trigger.family,
            TriggerFamily::ResponseCycle(_) | TriggerFamily::Session { .. }
        ) {
            self.set_membership(&key, ResultMembership::Retired);
            return Ok(SelectionOutcome::Updated);
        }
        self.apply_correction(activation, &key, &previous, observation, capture)
    }

    fn apply_correction<E>(
        &mut self,
        activation: &EffectiveActivation,
        key: &(BranchId, String),
        previous: &SeenResult,
        observation: &Observation,
        capture: &mut impl FnMut() -> Result<Option<crate::context::CurrentInventories>, E>,
    ) -> Result<SelectionOutcome, E> {
        if matches!(
            previous.membership,
            ResultMembership::Historical | ResultMembership::Retired | ResultMembership::Removed
        ) {
            return Ok(SelectionOutcome::Updated);
        }
        let selected = self
            .branches
            .get(&observation.branch)
            .is_some_and(|branch| branch.results.iter().any(|item| item.message.id == key.1));
        let coverage_complete = self
            .branches
            .get(&observation.branch)
            .is_none_or(|branch| branch.coverage_complete);
        let filter_result = evaluate_event_filters(&activation.compiled, observation);
        self.branches
            .entry(observation.branch.clone())
            .or_default()
            .last_filter_diagnostic = filter_result.as_ref().err().cloned();
        let filters_match = filter_result == Ok(true);
        let matches = if filters_match
            && let Some(mut context) = trigger_context(observation, coverage_complete)
        {
            if activation.compiled.current_inventory_demand().condition
                && let Some(inventories) = capture()?
            {
                inventories.merge_into(&mut context.value);
            }
            evaluate_trigger_condition(&activation.compiled, &context.value)
                == ConditionOutcome::Matched
        } else {
            false
        };
        if !matches {
            self.discard_result(&observation.branch, &key.1);
            self.set_membership(key, ResultMembership::Excluded);
            return Ok(filter_result
                .err()
                .map_or(SelectionOutcome::Updated, SelectionOutcome::FilterRejected));
        }
        if previous.membership == ResultMembership::Unknown && !selected {
            // An older checkpoint cannot distinguish an excluded result from a retired result.
            self.set_membership(key, ResultMembership::Retired);
            self.mark_coverage_gap(&observation.branch);
            return Ok(SelectionOutcome::WindowInvalidated);
        }
        if previous.native_order != observation.native_order || !selected {
            self.retire_window(&observation.branch);
            self.branches
                .entry(observation.branch.clone())
                .or_default()
                .coverage_complete = false;
            self.set_membership(key, ResultMembership::Retired);
            return Ok(SelectionOutcome::WindowInvalidated);
        }
        let message =
            observation_message(observation).expect("matching window evidence is a message");
        let selected = self
            .branches
            .get_mut(&observation.branch)
            .expect("selected branch exists")
            .results
            .iter_mut()
            .find(|item| item.message.id == key.1)
            .expect("selected result exists");
        selected.version = observation.version.clone();
        selected.message = message.clone();
        selected.complete = observation.complete;
        self.set_membership(key, ResultMembership::Selected);
        Ok(SelectionOutcome::Updated)
    }

    fn set_membership(&mut self, key: &(BranchId, String), membership: ResultMembership) {
        self.seen_results
            .get_mut(key)
            .expect("result was recorded")
            .membership = membership;
    }

    pub(crate) fn accepts_update(
        &self,
        branch: &BranchId,
        identity: &str,
        revision: u64,
        version: &EvidenceVersion,
    ) -> bool {
        self.seen_results
            .get(&(branch.clone(), identity.to_owned()))
            .is_none_or(|seen| {
                revision > seen.revision
                    || (revision == seen.revision
                        && seen.membership != ResultMembership::Removed
                        && *version != seen.version)
            })
    }

    pub(crate) fn is_normalized_duplicate(
        &self,
        branch: &BranchId,
        identity: &str,
        revision: u64,
        version: &EvidenceVersion,
    ) -> bool {
        self.seen_results
            .get(&(branch.clone(), identity.to_owned()))
            .is_some_and(|seen| {
                revision == seen.revision
                    && seen.membership != ResultMembership::Removed
                    && *version == seen.version
            })
    }

    /// Records a removal revision and returns whether it changes the source state.
    pub fn remove_result(&mut self, branch: &BranchId, identity: &str, revision: u64) -> bool {
        let key = (branch.clone(), identity.to_owned());
        if self.seen_results.get(&key).is_some_and(|seen| {
            revision < seen.revision
                || (revision == seen.revision && seen.membership == ResultMembership::Removed)
        }) {
            return false;
        }
        if !self.seen_results.contains_key(&key)
            && self.identity_count(branch) >= MAX_SEEN_RESULTS_PER_BRANCH
        {
            self.mark_identity_limit(branch);
            self.discard_result(branch, identity);
            return true;
        }
        let seen = self.seen_results.entry(key).or_insert_with(|| SeenResult {
            terminal_seen: true,
            revision,
            version: EvidenceVersion(format!("removed:{revision}")),
            native_order: NativeOrderKey(String::new()),
            membership: ResultMembership::Removed,
        });
        seen.terminal_seen = true;
        seen.revision = revision;
        seen.version = EvidenceVersion(format!("removed:{revision}"));
        seen.membership = ResultMembership::Removed;
        self.discard_result(branch, identity);
        true
    }

    fn identity_count(&self, branch: &BranchId) -> usize {
        self.seen_results
            .range((branch.clone(), String::new())..)
            .take_while(|((seen_branch, _), _)| seen_branch == branch)
            .count()
    }

    fn mark_identity_limit(&mut self, branch: &BranchId) {
        let selection = self.branches.entry(branch.clone()).or_default();
        selection.identity_limit_reached = true;
        selection.coverage_complete = false;
    }

    fn discard_result(&mut self, branch: &BranchId, identity: &str) {
        if let Some(selection) = self.branches.get_mut(branch) {
            selection
                .results
                .retain(|selected| selected.message.id != identity);
        }
    }

    /// Records incomplete membership without discarding usable published results.
    pub fn mark_coverage_gap(&mut self, branch: &BranchId) {
        let selection = self.branches.entry(branch.clone()).or_default();
        selection.coverage_complete = false;
    }

    /// Starts a new selection segment without restoring missing historical coverage.
    pub fn reset_segment(&mut self, branch: &BranchId) {
        self.retire_window(branch);
        self.mark_coverage_gap(branch);
    }

    /// Starts a fresh rolling window after the source proves coverage is complete.
    pub fn restore_coverage(&mut self, branch: &BranchId) {
        self.retire_window(branch);
        let selection = self.branches.entry(branch.clone()).or_default();
        selection.coverage_complete = true;
    }

    fn retire_window(&mut self, branch: &BranchId) {
        if let Some(selection) = self.branches.get_mut(branch) {
            selection.results.clear();
        }
        for ((seen_branch, _), seen) in self
            .seen_results
            .range_mut((branch.clone(), String::new())..)
        {
            if seen_branch != branch {
                break;
            }
            if seen.terminal_seen
                && !matches!(
                    seen.membership,
                    ResultMembership::Historical | ResultMembership::Removed
                )
            {
                seen.membership = ResultMembership::Retired;
            }
        }
    }
}

fn is_historical(activation: &EffectiveActivation, observation: &Observation) -> bool {
    observation.position <= activation.bootstrap_observation_position
        || if matches!(observation.kind, ObservationKind::ToolResult { .. }) {
            observation
                .first_terminal_publication
                .is_some_and(|cursor| cursor <= activation.bootstrap_publication)
        } else {
            observation
                .first_eligible_publication
                .is_none_or(|cursor| cursor <= activation.bootstrap_publication)
        }
}

fn observation_message(observation: &Observation) -> Option<&Message> {
    match &observation.kind {
        ObservationKind::UserMessage(message)
        | ObservationKind::AssistantMessage(message)
        | ObservationKind::ToolCall(message)
        | ObservationKind::ToolResult { message, .. } => Some(message),
        _ => None,
    }
}

fn trigger_context(observation: &Observation, coverage_complete: bool) -> Option<FrozenContext> {
    let mut context = observation.context.clone();
    let roots = context.value.as_object_mut()?;
    if let Some(message) = observation_message(observation) {
        roots.insert("message".into(), message.value.clone());
        roots.remove("turn");
    } else {
        roots.remove("message");
        if let ObservationKind::TurnEnd { turn } = &observation.kind {
            roots.insert("turn".into(), turn.clone());
        } else {
            roots.remove("turn");
        }
    }
    roots.remove("window");
    if let Some(session) = roots
        .get_mut("session")
        .and_then(serde_json::Value::as_object_mut)
    {
        session.insert(
            "source_coverage".into(),
            serde_json::json!(if coverage_complete {
                "complete"
            } else {
                "partial"
            }),
        );
    }
    Some(context)
}

fn matches_filters(
    check: &CompiledCheck,
    category: Option<ToolCategory>,
    status: ToolResultStatus,
) -> bool {
    check
        .trigger
        .tools
        .as_ref()
        .is_none_or(|tools| category.is_some_and(|category| tools.contains(&category)))
        && check
            .trigger
            .statuses
            .as_ref()
            .is_none_or(|statuses| statuses.contains(&status))
}

fn evaluate_message_filters(
    check: &CompiledCheck,
    message: &serde_json::Value,
) -> Result<(), FilterDiagnostic> {
    evaluate_attribution_filters(
        check.trigger.models.as_deref(),
        check.trigger.fast_mode,
        AttributionScope::Message,
        message,
    )?;
    evaluate_path_filters(
        check.trigger.paths.as_deref(),
        check.trigger.paths_ignore.as_deref(),
        message,
    )
}

fn evaluate_event_filters(
    check: &CompiledCheck,
    observation: &Observation,
) -> Result<bool, FilterDiagnostic> {
    match (&check.trigger.family, &observation.kind) {
        (
            TriggerFamily::Messages {
                tool_result: true, ..
            },
            ObservationKind::ToolResult {
                message,
                category,
                status,
            },
        ) => {
            if !parse_status(status)
                .is_some_and(|status| matches_filters(check, parse_category(category), status))
            {
                return Ok(false);
            }
            evaluate_message_filters(check, &message.value)?;
            Ok(true)
        }
        (
            TriggerFamily::Messages {
                user_message: Some(filters),
                ..
            },
            ObservationKind::UserMessage(message),
        )
        | (
            TriggerFamily::Messages {
                assistant_message: Some(filters),
                ..
            },
            ObservationKind::AssistantMessage(message),
        )
        | (
            TriggerFamily::Messages {
                tool_call: Some(filters),
                ..
            },
            ObservationKind::ToolCall(message),
        ) => {
            if filters.tools.as_ref().is_some_and(|tools| {
                !message.value["tool"]["category"]
                    .as_str()
                    .and_then(parse_category)
                    .is_some_and(|category| tools.contains(&category))
            }) {
                return Ok(false);
            }
            evaluate_scoped_filters(filters, AttributionScope::Message, &message.value)?;
            evaluate_path_filters(
                filters.paths.as_deref(),
                filters.paths_ignore.as_deref(),
                &message.value,
            )?;
            Ok(true)
        }
        (TriggerFamily::ResponseCycle(filters), ObservationKind::TurnEnd { turn }) => {
            if filters.statuses.as_ref().is_some_and(|statuses| {
                !turn["status"]
                    .as_str()
                    .and_then(parse_status)
                    .is_some_and(|status| statuses.contains(&status))
            }) {
                return Ok(false);
            }
            evaluate_scoped_filters(filters, AttributionScope::Turn, turn)?;
            Ok(true)
        }
        (
            TriggerFamily::Session {
                session_start: Some(filters),
                ..
            },
            ObservationKind::SessionStart { reason },
        ) => {
            if filters
                .reasons
                .as_ref()
                .is_some_and(|reasons| !reasons.contains(reason))
            {
                return Ok(false);
            }
            evaluate_scoped_filters(
                filters,
                AttributionScope::Session,
                &observation.context.value["session"],
            )?;
            Ok(true)
        }
        (
            TriggerFamily::Session {
                session_idle: Some(filters),
                ..
            },
            ObservationKind::SessionIdle { .. },
        ) => {
            evaluate_scoped_filters(
                filters,
                AttributionScope::Session,
                &observation.context.value["session"],
            )?;
            Ok(true)
        }
        _ => Ok(false),
    }
}

fn evaluate_scoped_filters(
    filters: &EventFilters,
    scope: AttributionScope,
    value: &serde_json::Value,
) -> Result<(), FilterDiagnostic> {
    evaluate_attribution_filters(filters.models.as_deref(), filters.fast_mode, scope, value)
}

fn parse_category(value: &str) -> Option<ToolCategory> {
    Some(match value {
        "shell" => ToolCategory::Shell,
        "read" => ToolCategory::Read,
        "search" => ToolCategory::Search,
        "edit" => ToolCategory::Edit,
        "write" => ToolCategory::Write,
        "delegate" => ToolCategory::Delegate,
        "skill" => ToolCategory::Skill,
        "mcp" => ToolCategory::Mcp,
        "other" => ToolCategory::Other,
        _ => return None,
    })
}

fn parse_status(value: &str) -> Option<ToolResultStatus> {
    Some(match value {
        "completed" => ToolResultStatus::Completed,
        "error" => ToolResultStatus::Error,
        "cancelled" => ToolResultStatus::Cancelled,
        "unknown" => ToolResultStatus::Unknown,
        _ => return None,
    })
}

fn candidate_fingerprint(
    activation: &crate::domain::ActivationId,
    branch: &BranchId,
    trigger: ObservationPosition,
    messages: &[Message],
    context: &FrozenContext,
    dependencies: &[EvidenceDependency],
) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(serde_json::to_vec(activation).expect("activation identity serializes"));
    update_string(&mut hasher, &branch.0);
    hasher.update(trigger.0.to_be_bytes());
    hasher.update(
        serde_json::to_vec(&messages.iter().map(|item| &item.value).collect::<Vec<_>>())
            .expect("JSON message values serialize"),
    );
    hasher.update(serde_json::to_vec(&context.value).expect("JSON context serializes"));
    for dependency in dependencies {
        match dependency {
            EvidenceDependency::Message { id, version } => {
                hasher.update([0]);
                update_string(&mut hasher, id);
                update_string(&mut hasher, &version.0);
            }
            EvidenceDependency::MembershipThrough(position) => {
                hasher.update([1]);
                hasher.update(position.0.to_be_bytes());
            }
            EvidenceDependency::Context { id, version } => {
                hasher.update([2]);
                update_string(&mut hasher, id);
                update_string(&mut hasher, &version.0);
            }
            EvidenceDependency::SourceEvidence { id, version } => {
                hasher.update([3]);
                update_string(&mut hasher, id);
                update_string(&mut hasher, &version.0);
            }
            EvidenceDependency::SessionActivity {
                attachment_epoch,
                activity_revision,
            } => {
                hasher.update([4]);
                update_string(&mut hasher, attachment_epoch);
                hasher.update(activity_revision.to_be_bytes());
            }
        }
    }
    hasher.finalize().into()
}

fn update_string(hasher: &mut Sha256, value: &str) {
    hasher.update((value.len() as u64).to_be_bytes());
    hasher.update(value.as_bytes());
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::compiler::compile;
    use crate::definition_lifecycle::EffectiveActivation;
    use crate::domain::{ActivationId, EvidenceVersion, Generation, NativeOrderKey};

    fn activation() -> EffectiveActivation {
        let source = "name: Retry\non:\n  tool_result:\n  window: {size: 3, min: 3}\ndecisions:\n  review:\n    input: {items: \"${{ window.messages }}\"}\n    output:\n      verdict:\n        type: choice\n        question: Is this useful?\n        criteria: {yes: Yes., no: No.}\nactions:\n  - type: status\n    input: {result: failure}\n";
        EffectiveActivation {
            id: ActivationId {
                scope: Default::default(),
                definition_id: "retry".to_owned(),
                generation: Generation(1),
            },
            revision: [1; 32],
            definition_scope: crate::discovery::DefinitionScope::Global,
            source_paths: vec![],
            project_root: None,
            compiled: std::sync::Arc::new(compile(source).unwrap()),
            bootstrap_observation_position: ObservationPosition(5),
            bootstrap_publication: Default::default(),
        }
    }

    fn result(position: u64) -> Observation {
        Observation {
            first_eligible_publication: None,
            position: ObservationPosition(position),
            branch: BranchId("main-session".to_owned()),
            native_order: NativeOrderKey(format!("native-{position}")),
            first_terminal_publication: None,
            version: EvidenceVersion(format!("v{position}")),
            kind: ObservationKind::ToolResult {
                message: Message {
                    id: format!("message-{position}"),
                    value: json!({"id": format!("message-{position}"), "tool": {"result": {"status": "error"}}}),
                },
                category: "shell".to_owned(),
                status: "error".to_owned(),
            },
            context: FrozenContext {
                value: json!({"session": {"requests": []}}),
                dependencies: vec![],
            },
            complete: true,
        }
    }

    fn update(position: u64, identity: &str, terminal: bool) -> Observation {
        let mut observation = result(position);
        if let ObservationKind::ToolResult { message, .. } = &mut observation.kind {
            message.id = identity.into();
            message.value["id"] = json!(identity);
        }
        if !terminal {
            observation.kind = ObservationKind::Other;
            observation.complete = false;
        }
        observation
    }

    fn window_ids(state: &SelectionState) -> Vec<&str> {
        state.branches[&BranchId("main-session".into())]
            .results
            .iter()
            .map(|item| item.message.id.as_str())
            .collect()
    }

    #[test]
    fn normalized_duplicate_admission_excludes_old_revisions_and_removal_tombstones() {
        let branch = BranchId("main-session".into());
        let version = EvidenceVersion("normalized-version".into());
        let changed_version = EvidenceVersion("changed-normalized-version".into());
        for membership in [
            ResultMembership::Unknown,
            ResultMembership::Historical,
            ResultMembership::Excluded,
            ResultMembership::Selected,
            ResultMembership::Retired,
            ResultMembership::Removed,
        ] {
            for terminal_seen in [false, true] {
                let mut state = SelectionState::default();
                state.seen_results.insert(
                    (branch.clone(), "member".into()),
                    SeenResult {
                        terminal_seen,
                        revision: 2,
                        version: version.clone(),
                        native_order: NativeOrderKey("native-order".into()),
                        membership,
                    },
                );
                assert!(!state.accepts_update(&branch, "member", 2, &version));
                assert_eq!(
                    state.is_normalized_duplicate(&branch, "member", 2, &version),
                    membership != ResultMembership::Removed
                );
                assert!(!state.is_normalized_duplicate(&branch, "member", 1, &version));
                assert!(!state.accepts_update(&branch, "member", 1, &version));
                assert!(!state.is_normalized_duplicate(&branch, "member", 2, &changed_version));
                assert_eq!(
                    state.accepts_update(&branch, "member", 2, &changed_version),
                    membership != ResultMembership::Removed
                );
                assert!(!state.is_normalized_duplicate(&branch, "member", 3, &version));
                assert!(state.accepts_update(&branch, "member", 3, &version));
                assert!(!state.is_normalized_duplicate(&branch, "unseen", 2, &version));
                assert!(state.accepts_update(&branch, "unseen", 2, &version));
            }
        }
    }

    #[test]
    fn bootstrap_excludes_old_results_and_window_rolls_at_its_limit() {
        let activation = activation();
        let mut state = SelectionState::default();
        assert_eq!(
            state.apply(&activation, &result(5)),
            SelectionOutcome::Updated
        );
        assert_eq!(
            state.apply(&activation, &result(6)),
            SelectionOutcome::Updated
        );
        assert_eq!(
            state.apply(&activation, &result(7)),
            SelectionOutcome::Updated
        );
        let SelectionOutcome::Candidate(first) = state.apply(&activation, &result(8)) else {
            panic!("minimum-sized window must create a candidate");
        };
        assert_eq!(
            first
                .messages
                .iter()
                .map(|message| message.id.as_str())
                .collect::<Vec<_>>(),
            ["message-6", "message-7", "message-8"]
        );
        let SelectionOutcome::Candidate(rolled) = state.apply(&activation, &result(9)) else {
            panic!("full rolling window must continue creating candidates");
        };
        assert_eq!(
            rolled
                .messages
                .iter()
                .map(|message| message.id.as_str())
                .collect::<Vec<_>>(),
            ["message-7", "message-8", "message-9"]
        );
        assert_ne!(first.fingerprint, rolled.fingerprint);
    }

    #[test]
    fn result_with_missing_source_fields_still_creates_a_partial_candidate() {
        let activation = activation();
        let mut state = SelectionState::default();
        for position in 6..=8 {
            let mut observation = result(position);
            if position == 8 {
                observation.complete = false;
                let ObservationKind::ToolResult { message, .. } = &mut observation.kind else {
                    unreachable!();
                };
                message.value["quality"] = json!({"arguments_available": false});
            }
            let outcome =
                state.apply_published(&activation, &format!("message-{position}"), 1, &observation);
            if position == 8 {
                let SelectionOutcome::Candidate(candidate) = outcome else {
                    panic!("missing optional source fields must not block review");
                };
                assert_eq!(
                    candidate.messages[2].value["quality"]["arguments_available"],
                    false
                );
            } else {
                assert_eq!(outcome, SelectionOutcome::Updated);
            }
        }
    }

    #[test]
    fn correction_updates_the_selected_identity_without_creating_a_trigger() {
        let activation = activation();
        let mut state = SelectionState::default();
        for position in 6..=8 {
            let observation = result(position);
            let outcome =
                state.apply_published(&activation, &format!("message-{position}"), 1, &observation);
            assert_eq!(
                matches!(outcome, SelectionOutcome::Candidate(_)),
                position == 8
            );
        }

        let mut correction = result(9);
        correction.native_order = NativeOrderKey("native-8".to_owned());
        correction.version = EvidenceVersion("v8-corrected".to_owned());
        let ObservationKind::ToolResult { message, .. } = &mut correction.kind else {
            unreachable!();
        };
        message.id = "message-8".to_owned();
        message.value = json!({"id": "message-8", "revision": 2});
        assert_eq!(
            state.apply_published(&activation, "message-8", 2, &correction),
            SelectionOutcome::Updated
        );

        let next = result(10);
        let SelectionOutcome::Candidate(candidate) =
            state.apply_published(&activation, "message-10", 1, &next)
        else {
            panic!("a later new result must trigger from the corrected window");
        };
        assert_eq!(candidate.messages[1].value["revision"], 2);
        assert!(
            candidate
                .dependencies
                .contains(&EvidenceDependency::Message {
                    id: "message-8".to_owned(),
                    version: EvidenceVersion("v8-corrected".to_owned()),
                })
        );
    }

    #[test]
    fn first_terminal_after_a_non_final_update_triggers_once() {
        let activation = activation();
        let mut state = SelectionState::default();
        for position in 6..=7 {
            state.apply_published(
                &activation,
                &format!("message-{position}"),
                1,
                &result(position),
            );
        }
        let mut pending = result(8);
        pending.kind = ObservationKind::Other;
        assert_eq!(
            state.apply_published(&activation, "message-8", 1, &pending),
            SelectionOutcome::Updated
        );
        let mut finalized = result(9);
        let ObservationKind::ToolResult { message, .. } = &mut finalized.kind else {
            unreachable!()
        };
        message.id = "message-8".into();
        assert!(matches!(
            state.apply_published(&activation, "message-8", 2, &finalized),
            SelectionOutcome::Candidate(_)
        ));
        finalized.position = ObservationPosition(10);
        finalized.version = EvidenceVersion("corrected".into());
        assert_eq!(
            state.apply_published(&activation, "message-8", 3, &finalized),
            SelectionOutcome::Updated
        );
        assert_eq!(state.branches[&finalized.branch].results.len(), 3);
    }

    #[test]
    fn same_revision_payload_change_updates_selected_evidence() {
        let activation = activation();
        let mut state = SelectionState::default();
        for position in 6..=8 {
            let observation = result(position);
            state.apply_published(&activation, &format!("message-{position}"), 1, &observation);
        }
        let mut changed = result(9);
        changed.native_order = NativeOrderKey("native-8".to_owned());
        changed.version = EvidenceVersion("same-revision-payload-v2".to_owned());
        let ObservationKind::ToolResult { message, .. } = &mut changed.kind else {
            unreachable!();
        };
        message.id = "message-8".to_owned();
        message.value = json!({"id": "message-8", "changed": true});
        assert_eq!(
            state.apply_published(&activation, "message-8", 1, &changed),
            SelectionOutcome::Updated
        );
        let next = result(10);
        let SelectionOutcome::Candidate(candidate) =
            state.apply_published(&activation, "message-10", 1, &next)
        else {
            panic!("new result must select the changed same-revision evidence");
        };
        assert_eq!(candidate.messages[1].value["changed"], true);
        assert!(
            candidate
                .dependencies
                .contains(&EvidenceDependency::Message {
                    id: "message-8".to_owned(),
                    version: EvidenceVersion("same-revision-payload-v2".to_owned()),
                })
        );
    }

    #[test]
    fn coverage_gap_keeps_usable_results_and_marks_candidates_partial() {
        let activation = activation();
        let mut state = SelectionState::default();
        let first = result(6);
        assert_eq!(
            state.apply_published(&activation, "message-6", 1, &first),
            SelectionOutcome::Updated
        );
        let branch = BranchId("main-session".to_owned());
        state.mark_coverage_gap(&branch);
        let during_gap = result(7);
        assert_eq!(
            state.apply_published(&activation, "message-7", 1, &during_gap),
            SelectionOutcome::Updated
        );
        let next = result(8);
        let SelectionOutcome::Candidate(candidate) =
            state.apply_published(&activation, "message-8", 1, &next)
        else {
            panic!("partial evidence must remain assessable");
        };
        assert_eq!(
            candidate.context.value["session"]["source_coverage"],
            "partial"
        );

        state.restore_coverage(&branch);
        for position in 9..=10 {
            let observation = result(position);
            assert_eq!(
                state
                    .apply_published(&activation, &format!("message-{position}"), 1, &observation,),
                SelectionOutcome::Updated
            );
        }
        let third = result(11);
        assert!(matches!(
            state.apply_published(&activation, "message-11", 1, &third),
            SelectionOutcome::Candidate(_)
        ));
    }

    #[test]
    fn segment_reset_retires_only_affected_window_and_resumes_with_partial_history() {
        let activation = activation();
        let mut state = SelectionState::default();
        for position in 6..=7 {
            state.apply_published(
                &activation,
                &format!("message-{position}"),
                1,
                &result(position),
            );
        }
        let mut other = update(8, "other-result", true);
        other.branch = BranchId("other-session".into());
        state.apply_published(&activation, "other-result", 1, &other);
        let other_before = state.branches[&other.branch].clone();

        let branch = BranchId("main-session".into());
        state.reset_segment(&branch);
        assert!(window_ids(&state).is_empty());
        assert!(!state.branches[&branch].coverage_complete);
        assert_eq!(state.branches[&other.branch], other_before);
        assert_eq!(
            state.seen_results[&(branch.clone(), "message-6".into())].membership,
            ResultMembership::Retired
        );

        let progress = crate::check_store::SourceProgress {
            selection: state,
            ..Default::default()
        };
        let mut state = crate::check_store::SourceProgress::decode(&progress.encode().unwrap())
            .unwrap()
            .selection;
        let mut correction = result(6);
        correction.position = ObservationPosition(9);
        correction.version = EvidenceVersion("corrected-old-segment".into());
        assert_eq!(
            state.apply_published(&activation, "message-6", 2, &correction),
            SelectionOutcome::Updated
        );
        assert!(window_ids(&state).is_empty());
        for position in 10..=11 {
            assert_eq!(
                state.apply_published(
                    &activation,
                    &format!("message-{position}"),
                    1,
                    &result(position)
                ),
                SelectionOutcome::Updated
            );
        }
        let SelectionOutcome::Candidate(candidate) =
            state.apply_published(&activation, "message-12", 1, &result(12))
        else {
            panic!("future results must form a new window without coverage restoration")
        };
        assert_eq!(
            window_ids(&state),
            ["message-10", "message-11", "message-12"]
        );
        assert_eq!(
            candidate.context.value["session"]["source_coverage"],
            "partial"
        );
        assert_eq!(state.branches[&other.branch], other_before);
    }

    #[test]
    fn unknown_category_is_usable_without_filters_and_never_matches_other() {
        let mut activation = activation();
        let mut observation = result(6);
        let ObservationKind::ToolResult { category, .. } = &mut observation.kind else {
            unreachable!()
        };
        category.clear();
        assert_eq!(
            SelectionState::default().apply(&activation, &observation),
            SelectionOutcome::Updated
        );
        std::sync::Arc::make_mut(&mut activation.compiled)
            .trigger
            .tools = Some(vec![ToolCategory::Other]);
        assert_eq!(
            SelectionState::default().apply(&activation, &observation),
            SelectionOutcome::Filtered
        );
        let ObservationKind::ToolResult { category, .. } = &mut observation.kind else {
            unreachable!()
        };
        *category = "other".into();
        assert_eq!(
            SelectionState::default().apply(&activation, &observation),
            SelectionOutcome::Updated
        );
    }

    #[test]
    fn false_and_unavailable_conditions_do_not_count_toward_window_minimum() {
        let mut activation = activation();
        std::sync::Arc::make_mut(&mut activation.compiled).condition =
            Some(crate::compiler::Condition::Expression(
                crate::expression::parse_expression("message.tool.result.content == 'keep'")
                    .unwrap(),
            ));
        let mut state = SelectionState::default();
        for (position, content) in [
            (6, Some("keep")),
            (7, Some("drop")),
            (8, None),
            (9, Some("keep")),
            (10, Some("keep")),
        ] {
            let mut observation = result(position);
            let ObservationKind::ToolResult { message, .. } = &mut observation.kind else {
                unreachable!()
            };
            if let Some(content) = content {
                message.value["tool"]["result"]["content"] = json!(content);
            }
            let selected =
                state.apply_published(&activation, &format!("message-{position}"), 1, &observation);
            match position {
                7 => assert_eq!(
                    selected,
                    SelectionOutcome::ConditionFiltered(ConditionOutcome::False)
                ),
                8 => assert_eq!(
                    selected,
                    SelectionOutcome::ConditionFiltered(ConditionOutcome::Unavailable)
                ),
                10 => {
                    let SelectionOutcome::Candidate(candidate) = selected else {
                        panic!("three eligible results must select")
                    };
                    assert_eq!(
                        candidate
                            .messages
                            .iter()
                            .map(|item| item.id.as_str())
                            .collect::<Vec<_>>(),
                        ["message-6", "message-9", "message-10"]
                    );
                }
                _ => assert_eq!(selected, SelectionOutcome::Updated),
            }
        }
    }

    #[test]
    fn correction_of_a_condition_filtered_result_preserves_the_retained_window() {
        let mut activation = activation();
        std::sync::Arc::make_mut(&mut activation.compiled).condition =
            Some(crate::compiler::Condition::Expression(
                crate::expression::parse_expression("message.tool.result.status == 'error'")
                    .unwrap(),
            ));
        let mut state = SelectionState::default();
        let mut filtered = result(6);
        let ObservationKind::ToolResult { message, .. } = &mut filtered.kind else {
            unreachable!()
        };
        message.value["tool"]["result"]["status"] = json!("completed");
        assert_eq!(
            state.apply_published(&activation, "message-6", 1, &filtered),
            SelectionOutcome::ConditionFiltered(ConditionOutcome::False)
        );
        for position in 7..=8 {
            state.apply_published(
                &activation,
                &format!("message-{position}"),
                1,
                &result(position),
            );
        }
        filtered.version = EvidenceVersion("corrected-filtered-result".into());
        filtered.position = ObservationPosition(9);
        assert_eq!(
            state.apply_published(&activation, "message-6", 2, &filtered),
            SelectionOutcome::Updated
        );
        let SelectionOutcome::Candidate(candidate) =
            state.apply_published(&activation, "message-10", 1, &result(10))
        else {
            panic!("a false correction must not discard earlier matching results");
        };
        assert_eq!(
            candidate
                .messages
                .iter()
                .map(|message| message.id.as_str())
                .collect::<Vec<_>>(),
            ["message-7", "message-8", "message-10"]
        );
    }

    #[test]
    fn repeated_progress_preserves_the_window_and_terminal_completion_order() {
        let activation = activation();
        let mut state = SelectionState::default();
        let lifecycle = [
            (6, "a", 1, false),
            (7, "b", 1, false),
            (8, "a", 2, false),
            (9, "b", 2, true),
            (10, "a", 3, false),
            (11, "a", 4, true),
            (12, "c", 1, false),
            (13, "c", 2, false),
            (14, "c", 3, false),
            (15, "c", 4, true),
        ];
        for (position, identity, revision, terminal) in lifecycle {
            let outcome = state.apply_published(
                &activation,
                identity,
                revision,
                &update(position, identity, terminal),
            );
            if position == 15 {
                let SelectionOutcome::Candidate(candidate) = outcome else {
                    panic!("three terminal identities must create a candidate")
                };
                assert_eq!(
                    candidate
                        .messages
                        .iter()
                        .map(|item| item.id.as_str())
                        .collect::<Vec<_>>(),
                    ["b", "a", "c"]
                );
                assert_eq!(
                    candidate.context.value["session"]["source_coverage"],
                    "complete"
                );
            } else {
                assert_eq!(outcome, SelectionOutcome::Updated);
            }
            if (9..11).contains(&position) {
                assert_eq!(window_ids(&state), ["b"]);
            } else if (11..15).contains(&position) {
                assert_eq!(window_ids(&state), ["b", "a"]);
            }
        }
    }

    #[test]
    fn order_changes_to_still_filtered_results_preserve_unrelated_results() {
        for filter in ["category", "status", "condition"] {
            let mut activation = activation();
            let check = std::sync::Arc::make_mut(&mut activation.compiled);
            match filter {
                "category" => check.trigger.tools = Some(vec![ToolCategory::Shell]),
                "status" => check.trigger.statuses = Some(vec![ToolResultStatus::Error]),
                "condition" => {
                    check.condition = Some(crate::compiler::Condition::Expression(
                        crate::expression::parse_expression(
                            "message.tool.result.status == 'error'",
                        )
                        .unwrap(),
                    ))
                }
                _ => unreachable!(),
            }
            let mut state = SelectionState::default();
            let mut filtered = update(6, "excluded", true);
            let ObservationKind::ToolResult {
                message,
                category,
                status,
            } = &mut filtered.kind
            else {
                unreachable!()
            };
            match filter {
                "category" => *category = "read".into(),
                "status" => *status = "completed".into(),
                "condition" => message.value["tool"]["result"]["status"] = json!("completed"),
                _ => unreachable!(),
            }
            state.apply_published(&activation, "excluded", 1, &filtered);
            for position in 7..=8 {
                state.apply_published(
                    &activation,
                    &format!("message-{position}"),
                    1,
                    &result(position),
                );
            }
            filtered.position = ObservationPosition(9);
            filtered.native_order = NativeOrderKey("changed-filtered-order".into());
            filtered.version = EvidenceVersion("changed-filtered-version".into());
            assert_eq!(
                state.apply_published(&activation, "excluded", 2, &filtered),
                SelectionOutcome::Updated,
                "{filter}"
            );
            let SelectionOutcome::Candidate(candidate) =
                state.apply_published(&activation, "message-10", 1, &result(10))
            else {
                panic!("filtered corrections must preserve the window: {filter}")
            };
            assert_eq!(
                candidate
                    .messages
                    .iter()
                    .map(|item| item.id.as_str())
                    .collect::<Vec<_>>(),
                ["message-7", "message-8", "message-10"]
            );
            assert_eq!(
                candidate.context.value["session"]["source_coverage"],
                "complete"
            );
        }
    }

    #[test]
    fn corrections_to_evicted_results_do_not_clear_or_reenter_the_window() {
        let activation = activation();
        let mut state = SelectionState::default();
        for position in 6..=9 {
            state.apply_published(
                &activation,
                &format!("message-{position}"),
                1,
                &result(position),
            );
        }
        let mut correction = result(6);
        correction.position = ObservationPosition(10);
        correction.version = EvidenceVersion("retired-correction".into());
        assert_eq!(
            state.apply_published(&activation, "message-6", 2, &correction),
            SelectionOutcome::Updated
        );
        assert_eq!(window_ids(&state), ["message-7", "message-8", "message-9"]);
        let SelectionOutcome::Candidate(candidate) =
            state.apply_published(&activation, "message-11", 1, &result(11))
        else {
            panic!("retired corrections must not restart window counting")
        };
        assert_eq!(
            candidate
                .messages
                .iter()
                .map(|item| item.id.as_str())
                .collect::<Vec<_>>(),
            ["message-8", "message-9", "message-11"]
        );
        assert_eq!(
            candidate.context.value["session"]["source_coverage"],
            "complete"
        );
    }

    #[test]
    fn newly_matching_correction_restarts_once_and_never_becomes_a_trigger() {
        let mut activation = activation();
        std::sync::Arc::make_mut(&mut activation.compiled)
            .trigger
            .tools = Some(vec![ToolCategory::Shell]);
        let mut state = SelectionState::default();
        let mut excluded = update(6, "excluded", true);
        let ObservationKind::ToolResult { category, .. } = &mut excluded.kind else {
            unreachable!()
        };
        *category = "read".into();
        state.apply_published(&activation, "excluded", 1, &excluded);
        for position in 7..=8 {
            state.apply_published(
                &activation,
                &format!("message-{position}"),
                1,
                &result(position),
            );
        }
        let mut correction = update(9, "excluded", true);
        correction.native_order = excluded.native_order;
        assert_eq!(
            state.apply_published(&activation, "excluded", 2, &correction),
            SelectionOutcome::WindowInvalidated
        );
        assert!(window_ids(&state).is_empty());
        state.apply_published(&activation, "message-10", 1, &result(10));
        correction.position = ObservationPosition(11);
        correction.version = EvidenceVersion("second-correction".into());
        assert_eq!(
            state.apply_published(&activation, "excluded", 3, &correction),
            SelectionOutcome::Updated
        );
        assert_eq!(window_ids(&state), ["message-10"]);
        state.apply_published(&activation, "message-12", 1, &result(12));
        let SelectionOutcome::Candidate(candidate) =
            state.apply_published(&activation, "message-13", 1, &result(13))
        else {
            panic!("fresh results must recover a partial window")
        };
        assert_eq!(
            candidate
                .messages
                .iter()
                .map(|item| item.id.as_str())
                .collect::<Vec<_>>(),
            ["message-10", "message-12", "message-13"]
        );
        assert_eq!(
            candidate.context.value["session"]["source_coverage"],
            "partial"
        );
    }

    #[test]
    fn correction_of_selected_result_order_invalidates_only_its_branch() {
        let activation = activation();
        let mut state = SelectionState::default();
        for position in 6..=8 {
            state.apply_published(
                &activation,
                &format!("message-{position}"),
                1,
                &result(position),
            );
        }
        let mut other_branch = update(9, "other-result", true);
        other_branch.branch = BranchId("other-session".into());
        state.apply_published(&activation, "other-result", 1, &other_branch);
        let correction = update(10, "message-8", true);
        assert_eq!(
            state.apply_published(&activation, "message-8", 2, &correction),
            SelectionOutcome::WindowInvalidated
        );
        assert!(window_ids(&state).is_empty());
        assert_eq!(state.branches[&other_branch.branch].results.len(), 1);
        assert!(state.branches[&other_branch.branch].coverage_complete);
    }

    #[test]
    fn historical_corrections_cannot_clear_live_results_or_trigger_again() {
        let activation = activation();
        let mut state = SelectionState::default();
        state.apply_published(&activation, "message-5", 1, &result(5));
        for position in 6..=7 {
            state.apply_published(
                &activation,
                &format!("message-{position}"),
                1,
                &result(position),
            );
        }
        let correction = update(8, "message-5", true);
        assert_eq!(
            state.apply_published(&activation, "message-5", 2, &correction),
            SelectionOutcome::Updated
        );
        assert_eq!(window_ids(&state), ["message-6", "message-7"]);
        assert!(matches!(
            state.apply_published(&activation, "message-9", 1, &result(9)),
            SelectionOutcome::Candidate(_)
        ));
    }

    #[test]
    fn first_terminal_publication_excludes_old_corrections_but_admits_pending_completion() {
        use crate::source::PublicationCursor;

        let mut activation = activation();
        activation.bootstrap_publication = PublicationCursor(100);
        let mut state = SelectionState::default();
        for first_terminal in [99, 100] {
            let identity = format!("old-{first_terminal}");
            if first_terminal == 100 {
                state.apply_published(&activation, &identity, 1, &update(6, &identity, false));
            }
            let mut correction = update(7, &identity, true);
            correction.first_terminal_publication = Some(PublicationCursor(first_terminal));
            assert_eq!(
                state.apply_published(&activation, &identity, 20, &correction),
                SelectionOutcome::Updated
            );
        }
        assert!(state.branches.is_empty());
        for (index, first_terminal) in [101, 102, 103].into_iter().enumerate() {
            let identity = format!("live-{index}");
            state.apply_published(&activation, &identity, 1, &update(4, &identity, false));
            state.apply_published(&activation, &identity, 2, &update(5, &identity, false));
            let mut terminal = update(8 + index as u64, &identity, true);
            terminal.first_terminal_publication = Some(PublicationCursor(first_terminal));
            let outcome = state.apply_published(&activation, &identity, 3, &terminal);
            assert_eq!(
                matches!(outcome, SelectionOutcome::Candidate(_)),
                index == 2
            );
        }
        assert_eq!(window_ids(&state), ["live-0", "live-1", "live-2"]);
    }

    #[test]
    fn removal_tombstones_reject_stale_updates_and_never_retrigger_reused_identity() {
        let activation = activation();
        let mut state = SelectionState::default();
        let branch = BranchId("main-session".into());
        state.apply_published(&activation, "a", 3, &update(6, "a", true));
        assert!(!state.remove_result(&branch, "a", 2));
        assert_eq!(window_ids(&state), ["a"]);
        assert!(state.remove_result(&branch, "a", 3));
        assert!(!state.remove_result(&branch, "a", 3));
        assert!(state.remove_result(&branch, "never-seen", 4));
        for (identity, revision) in [("a", 2), ("a", 3), ("a", 4), ("never-seen", 5)] {
            assert_eq!(
                state.apply_published(&activation, identity, revision, &update(7, identity, true)),
                SelectionOutcome::Updated
            );
            assert!(window_ids(&state).is_empty());
        }
        for position in 8..=10 {
            let outcome = state.apply_published(
                &activation,
                &format!("message-{position}"),
                1,
                &result(position),
            );
            assert_eq!(
                matches!(outcome, SelectionOutcome::Candidate(_)),
                position == 10
            );
        }
    }

    #[test]
    fn legacy_checkpoint_round_trip_keeps_compare_and_swap_bytes() {
        use crate::check_store::SourceProgress;

        let mut progress = SourceProgress::default();
        for position in 6..=9 {
            progress.selection.apply_published(
                &activation(),
                &format!("message-{position}"),
                1,
                &result(position),
            );
        }
        let mut legacy: serde_json::Value =
            serde_json::from_str(&progress.encode().unwrap()).unwrap();
        for entry in legacy["seen_results"].as_array_mut().unwrap() {
            entry[1].as_object_mut().unwrap().remove("membership");
        }
        let saved = legacy.to_string();
        let mut restored = SourceProgress::decode(&saved).unwrap();
        assert_eq!(restored.encode().unwrap(), saved);
        let mut correction = result(6);
        correction.position = ObservationPosition(10);
        correction.version = EvidenceVersion("legacy-retired-correction".into());
        assert_eq!(
            restored
                .selection
                .apply_published(&activation(), "message-6", 2, &correction),
            SelectionOutcome::WindowInvalidated
        );
        assert_eq!(
            window_ids(&restored.selection),
            ["message-7", "message-8", "message-9"]
        );
        let encoded = restored.encode().unwrap();
        assert_eq!(SourceProgress::decode(&encoded).unwrap(), restored);
    }

    #[test]
    fn retracted_or_newly_filtered_selected_results_preserve_other_window_members() {
        for retracted in [false, true] {
            let mut activation = activation();
            std::sync::Arc::make_mut(&mut activation.compiled)
                .trigger
                .statuses = Some(vec![ToolResultStatus::Error]);
            let mut state = SelectionState::default();
            for position in 6..=8 {
                state.apply_published(
                    &activation,
                    &format!("message-{position}"),
                    1,
                    &result(position),
                );
            }
            let mut correction = update(9, "message-7", !retracted);
            if let ObservationKind::ToolResult { status, .. } = &mut correction.kind {
                *status = "completed".into();
            }
            assert_eq!(
                state.apply_published(&activation, "message-7", 2, &correction),
                SelectionOutcome::Updated
            );
            assert_eq!(window_ids(&state), ["message-6", "message-8"]);
            let SelectionOutcome::Candidate(candidate) =
                state.apply_published(&activation, "message-10", 1, &result(10))
            else {
                panic!("a correction must remove only the excluded result")
            };
            assert_eq!(
                candidate
                    .messages
                    .iter()
                    .map(|item| item.id.as_str())
                    .collect::<Vec<_>>(),
                ["message-6", "message-8", "message-10"]
            );
            assert_eq!(
                candidate.context.value["session"]["source_coverage"],
                "complete"
            );
        }
    }

    #[test]
    fn coverage_restore_retires_old_exclusions_but_preserves_pending_completion() {
        let mut activation = activation();
        std::sync::Arc::make_mut(&mut activation.compiled)
            .trigger
            .tools = Some(vec![ToolCategory::Shell]);
        let mut state = SelectionState::default();
        let mut excluded = update(6, "excluded", true);
        let ObservationKind::ToolResult { category, .. } = &mut excluded.kind else {
            unreachable!()
        };
        *category = "read".into();
        state.apply_published(&activation, "excluded", 1, &excluded);
        state.apply_published(&activation, "pending", 1, &update(7, "pending", false));
        state.mark_coverage_gap(&excluded.branch);
        state.restore_coverage(&excluded.branch);
        state.apply_published(&activation, "message-8", 1, &result(8));
        assert_eq!(
            state.apply_published(&activation, "excluded", 2, &update(9, "excluded", true)),
            SelectionOutcome::Updated
        );
        state.apply_published(&activation, "pending", 2, &update(10, "pending", true));
        let SelectionOutcome::Candidate(candidate) =
            state.apply_published(&activation, "message-11", 1, &result(11))
        else {
            panic!("a pending first terminal must remain eligible after coverage restore")
        };
        assert_eq!(
            candidate
                .messages
                .iter()
                .map(|item| item.id.as_str())
                .collect::<Vec<_>>(),
            ["message-8", "pending", "message-11"]
        );
        assert_eq!(
            candidate.context.value["session"]["source_coverage"],
            "complete"
        );
    }

    #[test]
    fn identity_limit_fails_closed_and_survives_checkpoint_restore() {
        use crate::check_store::SourceProgress;

        let activation = activation();
        let branch = BranchId("main-session".into());
        let mut progress = SourceProgress::default();
        for index in 0..MAX_SEEN_RESULTS_PER_BRANCH {
            let identity = format!("result-{index}");
            progress.selection.apply_published(
                &activation,
                &identity,
                1,
                &update(index as u64 + 6, &identity, true),
            );
        }
        assert_eq!(
            progress.selection.identity_count(&branch),
            MAX_SEEN_RESULTS_PER_BRANCH
        );
        assert!(!progress.selection.branches[&branch].identity_limit_reached);

        let saved = progress.encode().unwrap();
        let mut restored = SourceProgress::decode(&saved).unwrap();
        let retired = update(6, "result-0", true);
        assert_eq!(
            restored
                .selection
                .apply_published(&activation, "result-0", 2, &retired),
            SelectionOutcome::Updated
        );
        let unseen = update(5000, "unseen", true);
        assert_eq!(
            restored
                .selection
                .apply_published(&activation, "unseen", 1, &unseen),
            SelectionOutcome::Updated
        );
        assert!(restored.selection.branches[&branch].identity_limit_reached);
        assert!(!restored.selection.branches[&branch].coverage_complete);
        assert_eq!(
            restored.selection.identity_count(&branch),
            MAX_SEEN_RESULTS_PER_BRANCH
        );

        let replay = update(5001, "result-0", true);
        assert_eq!(
            restored
                .selection
                .apply_published(&activation, "result-0", 3, &replay),
            SelectionOutcome::Updated
        );
        assert_eq!(
            restored.selection.identity_count(&branch),
            MAX_SEEN_RESULTS_PER_BRANCH
        );
    }
}
