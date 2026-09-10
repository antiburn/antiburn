use std::fmt;

use antiburn_local::insights::{DetectorId, ReportWindow};
use antiburn_local::model::AgentKind;
use antiburn_local::remediation::{FindingDisplay, RemediationUnavailableReason, SavingsValue};
use serde::{Deserialize, Serialize};

use crate::store::RemediationState;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BurnCheckTargetContext {
    pub environment_key: String,
    pub window: ReportWindow,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BurnCheckTarget {
    pub finding_id: String,
    pub action_id: String,
    pub finding: FindingDisplay,
    pub display: BurnCheckDisplayFacts,
    pub occurrences: usize,
    pub auto_fix: AutoFixAvailability,
    pub prompt_fix: PromptFixAvailability,
    pub watch: Option<WatchStatus>,
    pub coverage_limits: Vec<CoverageLimit>,
    pub sample_sessions: Vec<BurnCheckSampleSession>,
    pub expires_at_epoch: i64,
}

/// Internal session identity used only to mint an opaque renderer handle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BurnCheckSampleSession {
    pub environment_key: String,
    pub agent: String,
    pub session_id: String,
    pub observed_at_ms: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BurnCheckResourceKind {
    Session,
    Reasoning,
    Worker,
    McpServer,
    BuiltInTool,
    Skill,
    Model,
    Speed,
    Cache,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BurnCheckScopeKind {
    Global,
    Project,
    Session,
    Worker,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BurnCheckQuantityUnit {
    Tokens,
    Turns,
    Resources,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BurnCheckEstimateMethod {
    RepeatedContextAboveDepthCap,
    AssumedOutputReduction,
    WorkerModelPriceDifference,
    McpDefinitionExposure,
    BuiltInDefinitionReplication,
    InjectedSkillDocument,
    OldModelPriceDifference,
    FastTierPricePremium,
    CacheRehydrationPriceDifference,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BurnCheckVerificationLimit {
    FreshEvidenceFromSameSourceAndTarget,
    ExactPositiveControlRequired,
    CurrentEvidenceCannotProveFix,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BurnCheckDisplayFacts {
    pub resource_kind: BurnCheckResourceKind,
    pub resource_identity: Option<String>,
    pub current_value: Option<String>,
    pub replacement_value: Option<String>,
    pub scope_kind: BurnCheckScopeKind,
    pub quantity: Option<u64>,
    pub quantity_unit: Option<BurnCheckQuantityUnit>,
    pub observation_count: u64,
    pub first_observed_at_ms: i64,
    pub last_observed_at_ms: i64,
    pub estimate_method: Option<BurnCheckEstimateMethod>,
    #[serde(default)]
    pub estimated_opportunity: Option<SavingsValue>,
    pub verification_limit: BurnCheckVerificationLimit,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromptFixAvailability {
    Available,
    Unavailable(RemediationUnavailableReason),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AutoFixAvailability {
    Available,
    Unavailable(AutoFixUnavailableReason),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutoFixUnavailableReason {
    UnsupportedOrUnprovenTarget,
    ActiveWatch,
    SafetyCheckFailed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoverageLimit {
    CurrentPublishedEvidenceOnly,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WatchStatus {
    pub watch_id: String,
    pub origin: RemediationOrigin,
    pub lifecycle: RemediationState,
    pub verification: VerificationStatus,
    pub savings: SavingsStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemediationOrigin {
    Passive,
    Action,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "status",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum VerificationStatus {
    Reserved,
    Watching {
        #[serde(default)]
        reason: Option<VerificationReason>,
        #[serde(default)]
        method_revision: Option<u32>,
        #[serde(default)]
        evidence_revision: Option<String>,
    },
    Fixed {
        method_revision: u32,
        evidence_revision: String,
    },
    StillUnresolved {
        method_revision: u32,
        evidence_revision: String,
    },
    Recurred {
        method_revision: u32,
        evidence_revision: String,
    },
    RecoveryNeeded {
        reason: VerificationReason,
        #[serde(default)]
        checked_at_epoch: Option<i64>,
    },
    VerificationUnavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum VerificationReason {
    MissingPostBoundaryEvidence,
    WriteOutcomeUnknown,
    VerificationUnavailable,
    UnsupportedAgent,
    HomeUnavailable,
    PhysicalTargetChanged,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "status",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum SavingsStatus {
    Pending {
        #[serde(default)]
        method_revision: Option<u32>,
    },
    Unavailable,
    Unknown {
        reason: SavingsUnknownReason,
        method_revision: u32,
    },
    Known {
        method: SavingsMethod,
        method_revision: u32,
        pricing_revision: String,
        api_equivalent_cost_avoided_usd: f64,
        measured_through_ms: i64,
        recurrence_ms: Option<i64>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SavingsMethod {
    OldModelPriceDifference,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SavingsUnknownReason {
    MissingRates,
    MissingEvidence,
    MissingRevision,
    ArithmeticOverflow,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BurnCheckTargetList {
    pub targets: Vec<BurnCheckTarget>,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PromptFixResult {
    pub prompt: String,
    pub watch: WatchStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckPromptFixResult {
    pub prompt: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutoFixResult {
    pub watch_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutoFixReview {
    pub prepared_operation_id: String,
    pub expires_at_epoch: i64,
    pub agent: AgentKind,
    pub scope: BurnCheckScopeKind,
    pub setting: AutoFixSetting,
    pub current_value: String,
    pub proposed_value: String,
    pub effect: AutoFixEffect,
    pub side_effect: AutoFixSideEffect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutoFixSetting {
    Model,
    Reasoning,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutoFixEffect {
    FutureModelSelection,
    FutureReasoningEffort,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutoFixSideEffect {
    ModelBehaviorMayChange,
    ResponsesMayUseLessReasoning,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AggregateWins {
    pub wins: Vec<AggregateWin>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AggregateWin {
    pub finding_id: String,
    pub detector: DetectorId,
    pub origin: String,
    pub display: BurnCheckDisplayFacts,
    pub savings: AggregateSavings,
    pub starts_at_ms: i64,
    pub ends_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AggregateSavings {
    pub version: u32,
    pub token_savings: Option<u64>,
    pub api_equivalent_cost_avoided_usd: Option<f64>,
    pub improvement_count: Option<u64>,
    pub method: Option<BurnCheckEstimateMethod>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControllerError {
    TargetNotFound,
    TargetExpired,
    TargetChanged,
    Conflict,
    AutoFixUnavailable(AutoFixUnavailableReason),
    PromptUnavailable(RemediationUnavailableReason),
    CheckPromptUnavailable,
    ApplyFailed(crate::agent_config::ApplyError),
    RecoveryNeeded { watch_id: String },
    PersistenceFailed,
    Internal,
}

impl fmt::Display for ControllerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TargetNotFound => formatter.write_str("target_not_found"),
            Self::TargetExpired => formatter.write_str("target_expired"),
            Self::TargetChanged => formatter.write_str("target_changed"),
            Self::Conflict => formatter.write_str("conflict"),
            Self::AutoFixUnavailable(reason) => {
                write!(formatter, "auto_fix_unavailable:{reason:?}")
            }
            Self::PromptUnavailable(reason) => write!(formatter, "prompt_unavailable:{reason:?}"),
            Self::CheckPromptUnavailable => formatter.write_str("check_prompt_unavailable"),
            Self::ApplyFailed(_) => formatter.write_str("apply_failed"),
            Self::RecoveryNeeded { .. } => formatter.write_str("recovery_needed"),
            Self::PersistenceFailed => formatter.write_str("persistence_failed"),
            Self::Internal => formatter.write_str("internal_error"),
        }
    }
}

impl std::error::Error for ControllerError {}
