import type { CheckAvailability } from "../../../src/lib/checkAvailability"
import type {
  BurnCheckDetectorId,
  ChecksReportPayload,
  InstalledCheckDiagnosticPayload,
  InstalledCheckId,
  InstalledCheckReportPayload,
} from "../../../src/lib/insightsIpc"
import { CHECK_DEFINITIONS } from "../../../src/lib/presentation/checkDefinitions"
import { emitFixtureEvent } from "./event"

const availability: CheckAvailability = {
  revision: 0,
  yamlEnabled: false,
  checks: (Object.keys(CHECK_DEFINITIONS) as BurnCheckDetectorId[]).map((id) => ({
    id,
    enabled: true,
  })),
  configured: false,
  savedKey: false,
  error: null,
  usage: {
    inputTokens: 0,
    outputTokens: 0,
    confirmedCalls: 0,
    cacheHits: 0,
    unknownOutcomes: 0,
    estimatedUsd: "$0.00",
    lastUsedAtEpoch: null,
  },
}

export function fixtureCheckAvailability(): CheckAvailability {
  const checks = new URLSearchParams(window.location.search).get("checks")
  if (checks === "installed" || checks === "removed") {
    return {
      ...structuredClone(availability),
      yamlEnabled: true,
      checks: availability.checks.map(({ id }) => ({ id, enabled: false })),
      configured: true,
      installedChecks:
        checks === "installed"
          ? installedChecks().map(({ effectiveId, name, aliases, availability }) => ({
              effectiveId,
              name,
              aliases,
              availability,
            }))
          : [],
    }
  }
  return structuredClone(availability)
}

export function setFixtureCheckEnabled(args?: Record<string, unknown>): CheckAvailability {
  const check = availability.checks.find(({ id }) => id === args?.detector)
  if (!check || typeof args?.enabled !== "boolean") throw new Error("Unknown fixture check")
  if (check.enabled !== args.enabled) {
    check.enabled = args.enabled
    availability.revision += 1
    emitFixtureEvent("checks:availability-changed", {
      status: "updated",
      snapshot: fixtureCheckAvailability(),
    })
  }
  return fixtureCheckAvailability()
}

export function fixtureChecksReport(): ChecksReportPayload {
  const checks = new URLSearchParams(window.location.search).get("checks")
  if (checks === "installed" || checks === "removed") {
    return {
      evidenceSettled: true,
      windowSessions: 6,
      pendingEvidence: 0,
      deferredEvidence: 0,
      estimatedTokenBurnBasisPoints: null,
      categories: [],
      installedChecks: checks === "installed" ? installedChecks() : [],
    }
  }
  return {
    evidenceSettled: true,
    windowSessions: 128,
    pendingEvidence: 0,
    deferredEvidence: 0,
    estimatedTokenBurnBasisPoints: null,
    categories: availability.checks
      .filter(({ id, enabled }) => enabled && CHECK_DEFINITIONS[id].kind === "local")
      .map(({ id }) => ({
        id,
        finding: 0,
        clean: 128,
        unavailable: 0,
        estimatedTokenBurnBasisPoints: null,
      })),
  }
}

function installedCheck(
  effectiveId: InstalledCheckId,
  name: string,
  overrides: Partial<InstalledCheckReportPayload> = {},
): InstalledCheckReportPayload {
  return {
    effectiveId,
    name,
    aliases: [name],
    description: "Checks recorded session activity.",
    availability: { status: "available" },
    diagnostic: null,
    diagnostics: [],
    explanation: null,
    finding: 0,
    clean: 0,
    unavailable: 0,
    reviewFailed: 0,
    reviewedWithoutStatus: 0,
    hasStarted: false,
    checkingCount: 0,
    waitingForDataCount: 0,
    waitingForResultsCount: 0,
    waitingForEvents: null,
    agents: ["opencode"],
    targets: [],
    targetsTruncated: false,
    ...overrides,
  }
}

function installedChecks(): InstalledCheckReportPayload[] {
  const sourceIssue: InstalledCheckDiagnosticPayload = {
    code: "opencode-v2-capture-gap",
    severity: "error",
    title: "OpenCode live checks are not running",
    message:
      "Live activity is not available for this session. Keep OpenCode open, then rescan sessions. Live checks cannot recover missed activity. Saved results cover only their recorded input.",
    source: null,
    details: [
      { label: "Diagnostic code", value: "opencode-v2-capture-gap" },
      {
        label: "Reason",
        value: "The connection for live OpenCode activity is not active.",
      },
      {
        label: "Stored analysis",
        value: "Stored-session analysis does not require this live connection.",
      },
    ],
    affectedSessionCount: 1,
  }
  return [
    installedCheck("installed:first-assessment", "First check", { checkingCount: 1 }),
    installedCheck("installed:waiting", "Waiting for activity", {
      hasStarted: true,
      waitingForEvents: { event: "toolResult", count: 3, catchingUp: false },
    }),
    installedCheck("installed:connection-needed", "Connection unavailable", {
      availability: { status: "paused" },
      diagnostics: [
        {
          code: "provider-authentication",
          severity: "error",
          title: "Decision connection needed",
          message: "Review the decision connection in Settings.",
          source: null,
          details: [],
          affectedSessionCount: 1,
        },
      ],
    }),
    installedCheck("installed:saved-pass", "Saved pass", {
      hasStarted: true,
      clean: 1,
      checkingCount: 1,
      waitingForEvents: { event: "toolResult", count: 1, catchingUp: false },
    }),
    installedCheck("installed:saved-pass-explanation", "Saved pass with explanation", {
      hasStarted: true,
      clean: 1,
      checkingCount: 1,
      explanation: "The saved check found no concern.",
    }),
    installedCheck("installed:saved-failure", "Saved failure", {
      hasStarted: true,
      finding: 1,
      checkingCount: 1,
      unavailable: 1,
      diagnostics: [
        {
          ...sourceIssue,
          severity: "info",
        },
        sourceIssue,
        {
          ...sourceIssue,
          code: "source-unavailable",
          title: "Session journal unavailable",
          message: "Restore access to the session journal.",
          details: [],
        },
        {
          ...sourceIssue,
          code: "source-unavailable",
          title: "Session source unavailable",
          message: "Restore access to the session source.",
          details: [],
        },
      ],
      targets: [
        {
          id: "fixture-failed-assessment",
          title: "Repeated failure",
          agent: "opencode",
          projectName: "demo",
          sourcePath: ".antiburn/checks/retry.yaml",
          observation: "The same failed command ran again without a change.",
          evidenceAvailable: false,
          samples: [],
        },
      ],
    }),
  ]
}
