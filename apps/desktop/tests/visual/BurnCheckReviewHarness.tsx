import { createRoot } from "react-dom/client"

import { SessionStatusBar } from "../../src/components/session/SessionStatusBar"
import { TruncatedText } from "../../src/components/presentation/TruncatedText"
import { ChecksSummary } from "../../src/views/popover/ChecksView"
import type {
  ChecksCategoryPayload,
  SessionHygieneBadgePayload,
} from "../../src/lib/insightsIpc"
import { renderAgentIcon } from "../../src/lib/agentIcon"
import { checksPresentation } from "../../src/lib/presentation/checks"
import { sessionHygieneChecks } from "../../src/lib/presentation/sessionHygiene"

import "../../src/styles.css"
import "./BurnCheckReviewHarness.css"

const BADGE_IDS = [
  "sessionOverdepth",
  "modelOverthinking",
  "overpoweredSubagents",
  "obsoleteModel",
  "fastModeOveruse",
  "excessCacheRehydration",
] as const

function checks(statuses: SessionHygieneBadgePayload["status"][]) {
  return sessionHygieneChecks({
    evidenceState: "ready",
    badges: BADGE_IDS.map((id, index) => ({
      id,
      status: statuses[index] ?? "notAssessed",
      notAssessedReason: statuses[index] === "notAssessed" ? "incompleteEvidence" : null,
    })),
  })
}

function category(
  id: string,
  finding: number,
  clean: number,
  unavailable = 0,
): ChecksCategoryPayload {
  return {
    id,
    finding,
    clean,
    unavailable,
    estimatedTokenBurnBasisPoints: finding > 0 ? 1_500 : 0,
  }
}

const mixedSummary = checksPresentation({
  evidenceSettled: true,
  estimatedTokenBurnBasisPoints: 1_500,
  categories: [
    category("cacheChurn", 2, 8, 1),
    category("sessionsOverDepth", 0, 10),
    category("modelOverthinking", 0, 7),
  ],
})

const passedSummary = checksPresentation({
  evidenceSettled: true,
  estimatedTokenBurnBasisPoints: 0,
  categories: [category("sessionsOverDepth", 0, 10), category("modelOverthinking", 0, 7)],
})

function ReviewSessionCard({
  state,
  title,
  statuses,
}: {
  state: "rest" | "hover" | "selected"
  title: string
  statuses: SessionHygieneBadgePayload["status"][]
}) {
  return (
    <article className="session-card burn-check-review-card" data-review-state={state}>
      <div className="col-span-full">
        <SessionStatusBar
          checks={checks(statuses)}
          cost={{
            totalUsd: state === "hover" ? 2.4 : 1.82,
            figureLabel: "Estimated cost",
            models: ["GPT-5.6 Sol"],
            isHighCost: state === "hover",
          }}
        />
      </div>
      <div className="col-2 min-w-0">
        <TruncatedText className="type-body-large text-label" text={title} lines={2} />
      </div>
      <div className="col-2 flex min-w-0 items-center gap-x-1.5">
        <span className="inline-flex h-3.5 w-3.5 shrink-0 items-center justify-center">
          {renderAgentIcon("claude-code", 12, "cli", "neutral")}
        </span>
        <span className="truncate type-callout text-label-tertiary">
          <span className="font-medium">5.6-sol</span>
          <span className="type-caption"> high</span>
        </span>
      </div>
      <div className="col-2 flex justify-between type-callout text-label-tertiary">
        <span>antiburn</span>
        <span>{state === "selected" ? "18m ago" : "12m ago"}</span>
      </div>
    </article>
  )
}

function Harness() {
  return (
    <main className="burn-check-review">
      <section className="burn-check-review-main" aria-labelledby="review-main-title">
        <h1
          id="review-main-title"
          className="burn-check-review-heading type-headline text-label"
        >
          Main window · 1100px default · 340px session collection
        </h1>
        <div className="burn-check-review-session-list">
          <ReviewSessionCard
            state="rest"
            title="Skip imported Codex session noise"
            statuses={["clean", "clean", "clean", "clean", "clean", "clean"]}
          />
          <ReviewSessionCard
            state="hover"
            title="Refine context accounting and cache eviction across an unusually long imported session title"
            statuses={["finding", "clean", "clean", "clean", "clean", "notAssessed"]}
          />
          <ReviewSessionCard
            state="selected"
            title="Audit multi-agent handoff context"
            statuses={["finding", "finding", "finding", "clean", "clean", "clean"]}
          />
        </div>
      </section>

      <section aria-labelledby="review-popover-title">
        <h2
          id="review-popover-title"
          className="burn-check-review-heading type-headline text-label"
        >
          Popover · 380px
        </h2>
        <div className="burn-check-review-popover">
          <div className="burn-check-review-summary-list">
            <ChecksSummary
              active={false}
              presentation={mixedSummary}
              reportUnavailable={false}
              onPreview={() => undefined}
              onLeave={() => undefined}
            />
            <ChecksSummary
              active
              presentation={mixedSummary}
              reportUnavailable={false}
              onPreview={() => undefined}
              onLeave={() => undefined}
            />
            <ChecksSummary
              active={false}
              presentation={passedSummary}
              reportUnavailable={false}
              onPreview={() => undefined}
              onLeave={() => undefined}
            />
            <ChecksSummary
              active={false}
              presentation={null}
              reportUnavailable={true}
              onPreview={() => undefined}
              onLeave={() => undefined}
            />
          </div>
        </div>
      </section>
    </main>
  )
}

createRoot(document.getElementById("root")!).render(<Harness />)
