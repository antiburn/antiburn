import type {
  BurnCheckDetectorId,
  BurnCheckEvidenceItemPayload,
  BurnCheckTargetEvidencePayload,
  BurnCheckTargetPayload,
  ChecksReportPayload,
} from "../../../src/lib/insightsIpc"
import { skillOpportunityEvidence } from "../../../src/views/main-window/burn-checks/fixtures/skillOpportunityEvidence"

const detectors = [
  "overExploring",
  "ignoredInstructions",
  "scopeCreep",
  "skillOpportunities",
] as const
const path = `src/${"long-directory-name/".repeat(8)}parser.ts`
const command = `pnpm exec parser-check ${"--include=src/parser-boundary-tests ".repeat(12)}`

export function hasSmartCheckFixture() {
  return new URLSearchParams(window.location.search).get("checks") === "smart"
}

export function smartCheckReport(): ChecksReportPayload {
  const review = new URLSearchParams(window.location.search).get("review")
  if (review === "empty" || review === "blocked") {
    return {
      smartChecksAvailable: true,
      evidenceSettled: true,
      windowSessions: 1,
      pendingEvidence: 0,
      deferredEvidence: 0,
      estimatedTokenBurnBasisPoints: null,
      categories: [
        {
          id: "skillOpportunities",
          lifecycle: null,
          finding: 0,
          clean: 0,
          unavailable: 1,
          checking: false,
          sampled: true,
          reviewCoverage: {
            reviewed: 0,
            total: review === "empty" ? 0 : 4,
            uncertain: 0,
            pending: review === "empty" ? 0 : 4,
            continuing: false,
            skipped: review === "empty" ? 0 : 4,
            contextBlocked: review === "empty" ? 0 : 4,
          },
          estimatedTokenBurnBasisPoints: null,
        },
      ],
    }
  }
  return {
    smartChecksAvailable: true,
    evidenceSettled: true,
    windowSessions: 1,
    pendingEvidence: 0,
    deferredEvidence: 0,
    estimatedTokenBurnBasisPoints: null,
    categories: detectors.map((id) => ({
      id,
      lifecycle: "failing",
      finding: 1,
      clean: 0,
      unavailable: 0,
      checking: false,
      sampled: true,
      reviewCoverage: {
        reviewed: 2,
        total: 3,
        uncertain: 0,
        pending: 1,
        skipped: 1,
        contextBlocked: 0,
        continuing: false,
      },
      estimatedTokenBurnBasisPoints: null,
    })),
  }
}

export function smartCheckTarget(detector: BurnCheckDetectorId): BurnCheckTargetPayload {
  return {
    findingId: `fixture-${detector}`,
    actionId: detector,
    finding: {
      detector,
      agent: "opencode",
      sourceFormat: "openCodeSqliteV2",
      observation: "Selected comparison",
      labels: [],
      omitted: 0,
    },
    display: {
      resourceKind: detector === "skillOpportunities" ? "skill" : "session",
      resourceIdentity: detector === "skillOpportunities" ? "parser-review" : null,
      instructionTitle: detector === "ignoredInstructions" ? "Parser validation" : null,
      currentValue: null,
      replacementValue: null,
      scopeKind: "session",
      quantity: null,
      quantityUnit: null,
      observationCount: 1,
      firstObservedAtMs: 1000,
      lastObservedAtMs: 2000,
      estimateMethod: null,
      estimatedOpportunity: null,
      estimatedTokenBurnBasisPoints: null,
      verificationLimit: "currentEvidenceCannotProveFix",
    },
    occurrenceCount: 1,
    affectedSessionCount: 1,
    autoFix: { status: "unavailable", reason: "unsupportedOrUnprovenTarget" },
    promptFix: { status: "available" },
    watch: null,
    evidenceAvailable: true,
    coverageLimits: ["currentPublishedEvidenceOnly"],
    expiresAtEpoch: 9_999_999_999,
    samples: [
      {
        navigationHandle: "fixture-session",
        title: "Parser boundary review",
        agent: "opencode",
        surface: "cli",
        observedAtMs: 2000,
        timestamp: "2026-09-15T00:00:00Z",
        repo: "synthetic-parser",
        isActive: false,
        hasForkParent: false,
        forkChildCount: 0,
        cost: null,
        models: [],
        modelRuns: [],
        hygiene: {
          evidenceState: "ready",
          unusedResources: null,
          badges: [
            {
              id:
                detector === "ignoredInstructions"
                  ? "ignoredInstructions"
                  : detector === "scopeCreep"
                    ? "scopeCreep"
                    : detector === "overExploring"
                      ? "overExploring"
                      : "skillOpportunities",
              status: "finding",
              notAssessedReason: null,
            },
          ],
        },
      },
    ],
  }
}

function item(
  label: BurnCheckEvidenceItemPayload["label"],
  reference: string,
  excerpt: string,
  sourceLabel: string,
): BurnCheckEvidenceItemPayload {
  return {
    label,
    reference,
    excerpt,
    sourceLabel,
    observedAtMs: label === "instruction" ? null : 2000,
    startLine: null,
    endLine: null,
    explanation: "",
    limitation: null,
  }
}

export function smartCheckEvidence(actionId: unknown): BurnCheckTargetEvidencePayload {
  if (actionId === "skillOpportunities") return skillOpportunityEvidence
  const task = item(
    "context",
    "task",
    "Fix the parser timeout without changing billing.",
    "Requested task",
  )
  if (actionId === "overExploring") {
    return {
      status: "available",
      items: [
        task,
        item("observedAction", "request-1", "Read parser source", "Recorded read"),
        item(
          "observedAction",
          "result-1",
          `${"Selected parser source line\n".repeat(20)}${path}`,
          "Recorded read",
        ),
        item("observedAction", "request-2", "Read parser source again", "Recorded read"),
        item("observedAction", "result-2", "Selected later parser source", "Recorded read"),
      ],
      comparison: {
        reads: [
          {
            requestReference: "request-1",
            resultReference: "result-1",
            paths: [path],
            requestedExtent: { unit: "lines", offset: 10, limit: 30, end_inclusive: null },
            returnedExtent: { unit: "lines", offset: 10, limit: null, end_inclusive: 25 },
            resultStatus: "success",
          },
          {
            requestReference: "request-2",
            resultReference: "result-2",
            paths: [path],
            requestedExtent: { unit: "lines", offset: 10, limit: 30, end_inclusive: null },
            returnedExtent: { unit: "lines", offset: 10, limit: null, end_inclusive: 25 },
            resultStatus: "success",
          },
        ],
        explanation: {
          version: 1,
          relationship: "laterReadRepeatsEarlier",
          text: "The later parser read repeated the earlier returned region without helping the requested timeout fix.",
          references: ["task", "request-1", "result-1", "request-2", "result-2"],
        },
      },
    }
  }
  if (actionId === "scopeCreep")
    return {
      status: "available",
      items: [
        item("instruction", "task", task.excerpt, "Recorded user task"),
        item(
          "observedAction",
          "proposal",
          `Add a billing dashboard and then run ${command}`,
          "Recorded proposal",
        ),
      ],
      comparison: {
        observationKind: "proposal",
        reads: [],
        explanation: {
          version: 1,
          relationship: "separateObjective",
          text: "The requested work fixes the parser timeout. The proposed billing dashboard adds a separate objective.",
          references: ["task", "proposal"],
        },
      },
    }
  return {
    status: "available",
    items: [
      item(
        "instruction",
        "rule",
        "Use a focused parser test before publishing a parser change.",
        `${path} · Validation`,
      ),
      item("observedAction", "action", JSON.stringify({ command }), "Recorded command"),
      item("context", "earlier", "The parser tests have not run.", "Earlier recorded event"),
    ],
    comparison: {
      reads: [],
      explanation: {
        version: 1,
        relationship: "requirementConflict",
        text: "The instruction requires a focused parser test before publishing. The recorded command requests the broader parser check instead.",
        references: ["rule", "action", "earlier"],
      },
    },
  }
}
