import { invoke } from "@tauri-apps/api/core"
import { listen, type UnlistenFn } from "./tauriEvents"

import { hasShell } from "./ipc"
import type { ActivityEntryPayload } from "./ipc"
import type { LocalSessionIdentity } from "./types/session"

/* -------------------------------------------------------------------------
 * Local analysis payloads — mirrors of the types in `src-tauri/src/dto.rs`.
 * Counts, statuses, and structured reason identifiers only; no transcript
 * content anywhere in these shapes.
 * ---------------------------------------------------------------------- */

/** Why a category could not be assessed. Wording belongs to the reader. */
export type InsightsNotAssessedReason =
  | "noSessionsInWindow"
  | "capabilityMissing"
  | "incompleteEvidence"
  | "evidenceContractIncomplete"
  | "signalMissing"

export interface ChecksCategoryPayload {
  /** Agents with findings, or complete clean results when no finding exists. */
  agents?: string[]
  /** Stable category identifier, e.g. `sessionsOverDepth`. */
  id: BurnCheckDetectorId
  /** The current remediation state. Null means the category is not fully assessed. */
  lifecycle?: ChecksCategoryLifecycle | null
  /** Applicable sessions with a confirmed finding. */
  finding: number
  /** Applicable sessions with complete evidence and no finding. */
  clean: number
  /** Sessions without enough evidence for a finding or clean result. */
  unavailable: number
  /** Estimated avoidable tokens divided by total used tokens, in basis points from 0 to 10000. */
  estimatedTokenBurnBasisPoints: number | null
}

export type ChecksCategoryLifecycle = "failing" | "awaitingVerification" | "passing"

/** The bounded subset of the local report needed by All checks. */
export interface ChecksReportPayload {
  /** False while this report snapshot still has queued or running evidence work. */
  evidenceSettled: boolean
  /** Sessions the report window's denominator counts, regardless of evidence
   *  state. `pendingEvidence` is the subset of this total still queued or
   *  processing. */
  windowSessions: number
  /** Sessions with evidence queued or processing for this report window. */
  pendingEvidence: number
  /** The subset of `pendingEvidence` that waits for a retry backoff, for
   *  example a live session whose transcript changed during its check. */
  deferredEvidence: number
  /** Estimated avoidable tokens divided by total used tokens, in basis points from 0 to 10000. */
  estimatedTokenBurnBasisPoints: number | null
  /** Aggregate burn indexed by the active detector bit mask in canonical detector order. */
  estimatedTokenBurnBasisPointsByDetectorMask?: Array<number | null>
  categories: ChecksCategoryPayload[]
  /** Installed checks use their own identity and status projection. */
  installedChecks?: InstalledCheckReportPayload[]
}

export type InstalledCheckAvailability =
  | { status: "available" }
  | { status: "paused"; reason?: string | null }
  | { status: "invalid"; reason?: string | null }
  | { status: "unsupported"; reason?: string | null }

export type InstalledCheckId = `installed:${string}`
export type CheckId = BurnCheckDetectorId | InstalledCheckId

export function isInstalledCheckId(id: CheckId): id is InstalledCheckId {
  return id.startsWith("installed:")
}

export interface InstalledCheckDiagnosticSourcePayload {
  reference: string
  path: string
  line: number | null
  column: number | null
}

export interface InstalledCheckDiagnosticPayload {
  code: string
  severity: "error" | "info"
  title: string
  message: string
  source: InstalledCheckDiagnosticSourcePayload | null
  details: Array<{ label: string; value: string }>
  affectedSessionCount: number
}

export type InstalledCheckEvent =
  | "toolResult"
  | "userMessage"
  | "assistantMessage"
  | "toolCall"
  | "message"
  | "turnEnd"
  | "sessionStart"
  | "sessionIdle"
  | "sessionEvent"

/** Current native-window facts for one logical installed definition. */
export interface InstalledCheckReportPayload {
  effectiveId: InstalledCheckId
  name: string
  aliases: string[]
  description: string | null
  availability: InstalledCheckAvailability
  diagnostic: string | null
  diagnostics: InstalledCheckDiagnosticPayload[]
  explanation: string | null
  finding: number
  clean: number
  unavailable: number
  reviewFailed: number
  reviewedWithoutStatus: number
  hasStarted: boolean
  checkingCount: number
  waitingForDataCount: number
  waitingForResultsCount: number
  waitingForEvents: { event: InstalledCheckEvent; count: number; catchingUp: boolean } | null
  agents: string[]
  targets: InstalledCheckTargetPayload[]
  targetsTruncated: boolean
}

export interface InstalledCheckTargetPayload {
  id: string
  title: string
  agent: string
  projectName: string | null
  sourcePath: string | null
  observation: string | null
  evidenceAvailable: boolean
  samples: BurnCheckSamplePayload[]
}

export type BurnCheckDetectorId =
  | "sessionsOverDepth"
  | "modelOverthinking"
  | "overpoweredSubagents"
  | "unusedMcpServers"
  | "unusedBuiltInTools"
  | "unusedSkills"
  | "oldModelUsage"
  | "overuseOfFastMode"
  | "cacheChurn"

type BurnCheckEstimateMethod =
  | "repeatedContextAboveDepthCap"
  | "assumedOutputReduction"
  | "workerModelPriceDifference"
  | "mcpDefinitionExposure"
  | "builtInDefinitionReplication"
  | "injectedSkillDocument"
  | "oldModelPriceDifference"
  | "fastTierPricePremium"
  | "cacheRehydrationPriceDifference"

export interface BurnCheckEstimatedValuePayload {
  value: number
  unit:
    | "literalInputTokens"
    | "assumedOutputTokens"
    | "cacheClassTokens"
    | "apiEquivalentUsd"
    | "improvements"
}

type VerificationCoverageLimit =
  | "freshEvidenceFromSameSourceAndTarget"
  | "exactPositiveControlRequired"
  | "currentEvidenceCannotProveFix"

type BurnCheckSourceFormat =
  | "claudeJsonl"
  | "codexRolloutJsonl"
  | "openCodeJsonl"
  | "openCodeSqliteV1"
  | "openCodeSqliteV2"
  | "openCodeV2SessionExport"
  | "piV3Jsonl"
  | "cursorJsonl"
  | "cursorCliAgentJsonl"
  | "cursorCliStoreDb"
  | "cursorChatStoreDb"
  | "cursorIdeComposer"
  | "cursorLegacyChatJson"
  | "antigravityJson"
  | "antigravityBrainJsonl"
  | "antigravityCascadeJson"
  | "antigravityWorkspaceChatJson"
  | "antigravitySqlite"
  | "copilotCliJsonl"
  | "copilotIdeChatJson"
  | "clineSessionJson"
  | "kiroSessionJson"
  | "kiroChat"
  | "ampThreadJson"
  | "ampFileChanges"
  | "windsurfWorkspaceJson"
  | "windsurfMirrorJson"
  | "windsurfCascadeProtobuf"
  | "devinLocalSqlite"
  | "uncharacterized"

interface BurnCheckFindingPayload {
  detector: BurnCheckDetectorId
  agent: string
  sourceFormat: BurnCheckSourceFormat
  observation: string
  labels: string[]
  omitted: number
}

interface BurnCheckDisplayFactsPayload {
  resourceKind:
    | "session"
    | "reasoning"
    | "worker"
    | "mcpServer"
    | "builtInTool"
    | "skill"
    | "model"
    | "speed"
    | "cache"
  resourceIdentity: string | null
  currentValue: string | null
  replacementValue: string | null
  scopeKind: "global" | "project" | "session" | "worker"
  quantity: number | null
  quantityUnit: "tokens" | "turns" | "resources" | null
  observationCount: number
  firstObservedAtMs: number
  lastObservedAtMs: number
  estimateMethod: BurnCheckEstimateMethod | null
  /** Present only when the backend returns a reviewed value and unit. */
  estimatedOpportunity: BurnCheckEstimatedValuePayload | null
  /** Complete attributed target tokens divided by the report token denominator. */
  estimatedTokenBurnBasisPoints: number | null
  verificationLimit: VerificationCoverageLimit
}

type AutoFixUnavailableReason =
  "unsupportedOrUnprovenTarget" | "activeWatch" | "safetyCheckFailed" | "targetNotFound"

type AutoFixAvailabilityPayload =
  { status: "available" } | { status: "unavailable"; reason: AutoFixUnavailableReason }

type PromptFixAvailabilityPayload =
  { status: "available" } | { status: "unavailable"; reason: PromptFixUnavailableReason }

type BurnCheckWatchLifecycle =
  | "reserved"
  | "writing"
  | "recoveryNeeded"
  | "waitingForPromptUse"
  | "watching"
  | "fixed"
  | "recurred"

type BurnCheckVerificationReason =
  | "missingPostBoundaryEvidence"
  | "writeOutcomeUnknown"
  | "verificationUnavailable"
  | "unsupportedAgent"
  | "homeUnavailable"
  | "physicalTargetChanged"

type BurnCheckVerificationPayload =
  | { status: "reserved" }
  | {
      status: "watching"
      reason?: BurnCheckVerificationReason
      methodRevision?: number
      evidenceRevision?: string
    }
  | { status: "fixed"; methodRevision: number; evidenceRevision: string }
  | { status: "stillUnresolved"; methodRevision: number; evidenceRevision: string }
  | { status: "recurred"; methodRevision: number; evidenceRevision: string }
  | {
      status: "recoveryNeeded"
      reason: BurnCheckVerificationReason
      checkedAtEpoch?: number
    }
  | { status: "verificationUnavailable" }

type BurnCheckSavingsUnknownReason =
  "missingRates" | "missingEvidence" | "missingRevision" | "arithmeticOverflow"

type BurnCheckSavingsPayload =
  | { status: "pending"; methodRevision?: number }
  | { status: "unavailable" }
  | {
      status: "unknown"
      reason: BurnCheckSavingsUnknownReason
      methodRevision: number
    }
  | {
      status: "known"
      method: "oldModelPriceDifference"
      methodRevision: number
      pricingRevision: string
      apiEquivalentCostAvoidedUsd: number
      measuredThroughMs: number
      recurrenceMs: number | null
    }

interface BurnCheckWatchPayload {
  watchId: string
  origin: "passive" | "action"
  lifecycle: BurnCheckWatchLifecycle
  verification: BurnCheckVerificationPayload
  savings: BurnCheckSavingsPayload
}

export interface BurnCheckTargetPayload {
  findingId: string
  actionId: string
  finding: BurnCheckFindingPayload
  display: BurnCheckDisplayFactsPayload
  occurrenceCount: number
  affectedSessionCount?: number | null
  projectName?: string | null
  projectLocation?: string | null
  /** Full local directory for explicit folder actions. Never send to analytics. */
  projectPath?: string | null
  configFile?: string | null
  autoFix: AutoFixAvailabilityPayload
  promptFix: PromptFixAvailabilityPayload
  watch: BurnCheckWatchPayload | null
  evidenceAvailable: boolean
  coverageLimits: "currentPublishedEvidenceOnly"[]
  samples: BurnCheckSamplePayload[]
  expiresAtEpoch: number
}

export interface BurnCheckEvidenceItemPayload {
  label: "instruction" | "observedAction" | "context"
  sourceLabel: string
  reference: string
  observedAtMs: number | null
  startLine: number | null
  endLine: number | null
  excerpt: string
  explanation: string
  limitation: string | null
}

export interface BurnCheckTargetEvidencePayload {
  status: "available" | "unavailable"
  items: BurnCheckEvidenceItemPayload[]
  comparison?: BurnCheckEvidenceComparisonPayload
  occurrences?: {
    findingId: string
    status: "available" | "unavailable"
    items: BurnCheckEvidenceItemPayload[]
    comparison?: BurnCheckEvidenceComparisonPayload
  }[]
}

interface BurnCheckEvidenceComparisonPayload {
  sourceRanges?: {
    reference: string
    startByte: number
    endByte: number
    rangeSource: string
  }[]
  observationKind?: "proposal" | "attempt" | "recorded"
  explanation?: {
    version: 1
    relationship: string
    text: string
    references: string[]
  }
  reads: {
    requestReference: string
    resultReference?: string
    paths: string[]
    requestedExtent: BurnCheckReadExtentPayload
    returnedExtent?: BurnCheckReadExtentPayload
    resultStatus?: "success" | "failed" | "unknown"
  }[]
}

interface BurnCheckReadExtentPayload {
  unit: "lines" | "bytes" | "unknown"
  offset: number | null
  limit: number | null
  end_inclusive: number | null
}

/** Bounded display metadata plus an opaque, expiring route to one local session. */
export interface BurnCheckSamplePayload extends Omit<
  ActivityEntryPayload,
  "sessionId" | "wslDistro" | "totalTokens"
> {
  navigationHandle: string
  title: string
  agent: string
  surface: "cli" | "ide_desktop" | "unknown"
  observedAtMs: number
  hygiene: SessionHygienePayload
}

export type OpenBurnCheckSampleOutcome =
  | { outcome: "opened" }
  | { outcome: "deleted" }
  | { outcome: "expired" }
  | { outcome: "unavailable" }

export interface BurnCheckTargetListPayload {
  targets: BurnCheckTargetPayload[]
  samples: BurnCheckSamplePayload[]
  truncated: boolean
}

export interface AutoFixReviewPayload {
  preparedOperationId: string
  expiresAtEpoch: number
  agent: string
  scope: "global" | "project" | "session" | "worker"
  setting:
    | "model"
    | "reasoning"
    | "compaction"
    | "subagentModel"
    | "mcpServer"
    | "builtInTool"
    | "skill"
    | "fastMode"
  configFile: string
  selectorLabel: string
  currentValue: string
  proposedValue: string
  behaviorOverrideWarning: boolean
  effect:
    | "futureModelDefault"
    | "skillAdvertising"
    | "modelSelection"
    | "reasoningEffort"
    | "sessionCompaction"
    | "workerModelSelection"
    | "mcpAvailability"
    | "toolAvailability"
    | "skillAvailability"
    | "serviceTierSelection"
  sideEffect:
    | "activeSessionsKeepTheirModel"
    | "explicitSkillUseRemainsAvailable"
    | "modelBehaviorMayChange"
    | "responsesMayUseLessReasoning"
    | "earlierSessionSummarization"
    | "workerBehaviorMayChange"
    | "serverWillNotBeAvailable"
    | "toolWillNotBeAvailable"
    | "skillWillNotBeAvailable"
    | "responsesMayTakeLonger"
}

export type PrepareAutoFixBurnCheckTargetOutcome =
  | { outcome: "reviewReady"; review: AutoFixReviewPayload }
  | { outcome: "stale" }
  | { outcome: "expired" }
  | { outcome: "conflict" }
  | { outcome: "unavailable"; reason: AutoFixUnavailableReason }

export type ApplyPreparedBurnCheckOperationOutcome =
  | { outcome: "appliedAwaitingVerification"; watchId: string }
  | { outcome: "appliedVerificationUnavailable"; watchId: string }
  | { outcome: "recoveryNeeded"; watchId: string }
  | { outcome: "stale" }
  | { outcome: "expired" }
  | { outcome: "conflict" }
  | { outcome: "unavailable"; reason: AutoFixUnavailableReason }

type PromptFixUnavailableReason =
  | "targetNotFound"
  | "promptSizeLimit"
  | "essentialIdentityUnavailable"
  | "protectedBuiltInTool"
  | "deferredAgent"
  | "unsupportedSourceFormat"
  | "checkUnsupportedForAgent"

export type CopyPromptFixBurnCheckTargetOutcome =
  | { outcome: "promptReady"; prompt: string; watch: BurnCheckWatchPayload | null }
  | { outcome: "stale" }
  | { outcome: "expired" }
  | { outcome: "unavailable"; reason: PromptFixUnavailableReason }

export type CopyPromptFixBurnCheckOutcome =
  { outcome: "promptReady"; prompt: string } | { outcome: "unavailable" }

export interface AggregateWinPayload {
  findingId: string
  remediationCycleId: string
  detector: BurnCheckDetectorId
  origin: "passive" | "action"
  display: BurnCheckDisplayFactsPayload
  savings: {
    status: BurnCheckSavingsPayload
    tokenSavings: number | null
    apiEquivalentCostAvoidedUsd: number | null
    improvementCount: number | null
    method: BurnCheckEstimateMethod | null
  }
  verifiedBoundaryMs: number
  startsAtMs: number
  endsAtMs: number
}

export interface AggregateWinsPayload {
  wins: AggregateWinPayload[]
}

export type SessionHygieneBadgeId =
  | "sessionOverdepth"
  | "modelOverthinking"
  | "overpoweredSubagents"
  | "obsoleteModel"
  | "fastModeOveruse"
  | "excessCacheRehydration"

export type SessionHygieneFindingEvidence =
  | {
      kind: "sessionOverdepth"
      maxRequestContextTokens: number
      depthCapTokens: number
    }
  | {
      kind: "modelOverthinking"
      tiers: Array<{
        tier: string
        mainLoopTurns: number
        delegatedTurns: number
      }>
    }
  | {
      kind: "overpoweredSubagents"
      mainModels: string[]
      delegatedModels: string[]
    }
  | {
      kind: "obsoleteModel"
      models: Array<{
        model: string
        replacement: string
      }>
    }
  | {
      kind: "fastModeOveruse"
      delegatedTurns: number
    }
  | {
      kind: "excessCacheRehydration"
      repeatedTokens: number
      paidTokens: number
      thresholdMultiple: number
    }

export interface SessionHygieneBadgePayload {
  id: SessionHygieneBadgeId
  status: "finding" | "clean" | "notAssessed"
  notAssessedReason: InsightsNotAssessedReason | null
  /** Which vendor billing mechanism backs an `excessCacheRehydration`
   *  verdict. Absent for every other badge and for old evidence with no
   *  `repeated_context` marker. */
  accounting?: "cacheWrite" | "uncachedInput"
  /** The stored facts that caused a finding. Absent for every other status. */
  findingEvidence?: SessionHygieneFindingEvidence
}

export type SessionHygieneEvidenceState =
  "pending" | "processing" | "ready" | "unsupported" | "failed" | "stale" | "activelyGrowing"

/** One resource that sat in every request's context and was never called. */
export interface UnusedResource {
  name: string
  costUsd: number | null
}

/** Priced idle context for one session, grouped by resource kind. */
interface SessionUnusedResources {
  mcpServers: UnusedResource[]
  builtInTools: UnusedResource[]
  skills: UnusedResource[]
}

export interface SessionHygienePayload {
  badges: SessionHygieneBadgePayload[]
  evidenceState: SessionHygieneEvidenceState
  unusedResources: SessionUnusedResources | null
}

/** The real local detector results and bounded session navigation targets. */
export async function getChecksReport(consumerId: string): Promise<ChecksReportPayload | null> {
  if (!hasShell()) return null
  return invoke<ChecksReportPayload>("get_checks_report", { consumerId })
}

export function getInstalledCheckEvidence(
  targetId: string,
): Promise<BurnCheckTargetEvidencePayload> {
  return invoke<BurnCheckTargetEvidencePayload>("get_installed_check_evidence", { targetId })
}

export function openInstalledCheckDefinition(sourceReference: string): Promise<void> {
  return invoke<void>("open_installed_check_definition", { sourceReference })
}

/** Lists current exact targets for one detector in the native thirty-day context. */
export async function listBurnCheckTargets(
  detector: BurnCheckDetectorId,
): Promise<BurnCheckTargetListPayload | null> {
  if (!hasShell()) return null
  return invoke<BurnCheckTargetListPayload>("list_burn_check_targets", {
    detector,
  })
}

/** Prepares one exact automatic change for semantic review. */
export async function prepareAutoFixBurnCheckTarget(
  actionId: string,
): Promise<PrepareAutoFixBurnCheckTargetOutcome | null> {
  if (!hasShell()) return null
  return invoke<PrepareAutoFixBurnCheckTargetOutcome>("prepare_auto_fix_burn_check_target", {
    actionId,
  })
}

/** Applies only the operation returned by the matching review. */
export async function applyPreparedBurnCheckOperation(
  preparedOperationId: string,
): Promise<ApplyPreparedBurnCheckOperationOutcome | null> {
  if (!hasShell()) return null
  return invoke<ApplyPreparedBurnCheckOperationOutcome>("apply_prepared_burn_check_operation", {
    preparedOperationId,
  })
}

/** Builds a current prompt for one stable target identity. */
export async function copyPromptFixBurnCheckTarget(
  detector: BurnCheckDetectorId,
  findingId: string,
): Promise<CopyPromptFixBurnCheckTargetOutcome | null> {
  if (!hasShell()) return null
  return invoke<CopyPromptFixBurnCheckTargetOutcome>("copy_prompt_fix_burn_check_target", {
    detector,
    findingId,
  })
}

/** Builds a current bounded prompt for one check. */
export async function copyPromptFixBurnCheck(
  detector: BurnCheckDetectorId,
): Promise<CopyPromptFixBurnCheckOutcome | null> {
  if (!hasShell()) return null
  return invoke<CopyPromptFixBurnCheckOutcome>("copy_prompt_fix_burn_check", { detector })
}

/** Prepares one bounded prompt for all selected exact targets in a check. */
export async function copyPromptFixBurnCheckTargets(
  detector: BurnCheckDetectorId,
  findingIds: string[],
): Promise<CopyPromptFixBurnCheckOutcome | null> {
  if (!hasShell()) return null
  return invoke<CopyPromptFixBurnCheckOutcome>("copy_prompt_fix_burn_check_targets", {
    detector,
    findingIds,
  })
}

/** Reads bounded durable wins independently from current findings. */
export async function getBurnCheckAggregateWins(): Promise<AggregateWinsPayload | null> {
  if (!hasShell()) return null
  return invoke<AggregateWinsPayload>("get_burn_check_aggregate_wins")
}

/** Resolve an opaque sample handle and route through the standard session controller. */
export async function openBurnCheckSample(
  navigationHandle: string,
): Promise<OpenBurnCheckSampleOutcome | null> {
  if (!hasShell()) return null
  return invoke<OpenBurnCheckSampleOutcome>("open_burn_check_sample", { navigationHandle })
}

/** Stop a Checks reduction that no visible popover still needs. */
export async function cancelChecksReport(consumerId: string): Promise<void> {
  if (!hasShell()) return
  await invoke("cancel_checks_report", { consumerId })
}

/** Run after the evidence worker publishes every item in its current queue. */
export async function onChecksReportChanged(handler: () => void): Promise<UnlistenFn> {
  return listen("checks:report-changed", handler)
}

/** The hygiene badges reduced from a bounded set of stored evidence rows. */
export async function getSessionHygiene(
  sessions: readonly LocalSessionIdentity[],
): Promise<SessionHygienePayload[] | null> {
  if (!hasShell()) return null
  return invoke<SessionHygienePayload[]>("get_session_hygiene", {
    sessions: sessions.map((session) => ({
      agent: session.agent,
      sessionId: session.sessionId,
      wslDistro: session.wslDistro ?? null,
      ...(session.remoteHostId ? { remoteHostId: session.remoteHostId } : {}),
    })),
  })
}
