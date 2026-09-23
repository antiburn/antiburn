import { FolderPlus } from "lucide-react"
import { useCallback, useState, useSyncExternalStore } from "react"

import { cn } from "../../../lib/cn"
import type { ChecksCategoryPayload } from "../../../lib/insightsIpc"
import { getScanStatus, openSettingsWindow } from "../../../lib/ipc"
import { agentDisplayName } from "../../../lib/presentation/agents"
import { checksPresentation } from "../../../lib/presentation/checks"
import { scanStatusStore, withKnownAgents } from "../../../lib/scanStatusStore"
import {
  formatSnoozeUntil,
  snoozedDetectorIds,
  useSnoozedBurnChecks,
} from "../../../lib/snoozedBurnChecks"
import { PushButton } from "../../../components/ui/PushButton"
import { checkRowPresentation, CHECK_UI } from "../../checks/checkUi"
import type { BurnChecksSession, BurnChecksSnapshot } from "../BurnChecksSession"
import { CheckDetailActions, CHECK_SENTENCES } from "../burn-checks/BurnCheckDetail"
import { RemindLaterAction } from "../burn-checks/RemindLaterAction"

export function Waiting({ children }: { children: string }) {
  return (
    <p role="status" className="type-callout text-label-secondary">
      {children}
    </p>
  )
}

export function SourcesStep() {
  const status = useSyncExternalStore(
    scanStatusStore.subscribe,
    scanStatusStore.getSnapshot,
    scanStatusStore.getSnapshot,
  )
  const [fetched, setFetched] = useState(false)
  // Only `get_scan_status` fills the agent list, and a scan event that lands
  // during the store's first load discards that load. The step asks for the
  // status again when it mounts, so the list is never left empty.
  const fetchAgents = useCallback((node: HTMLElement | null) => {
    if (!node) return
    void getScanStatus()
      .catch(() => null)
      .then((fresh) => {
        if (fresh) scanStatusStore.set(withKnownAgents(fresh))
        setFetched(true)
      })
  }, [])
  if (!status || (!fetched && status.agents.length === 0))
    return (
      <div ref={fetchAgents}>
        <Waiting>Looking for your agents…</Waiting>
      </div>
    )
  const agents = status.agents
    .filter((agent) => agent.sessionsSeen > 0)
    .sort((left, right) => right.sessionsSeen - left.sessionsSeen)
  return (
    <div className="flex max-w-xl flex-col gap-(--space-lg)">
      {agents.length === 0 ? (
        <p className="type-body text-label-secondary">
          No agent sessions found yet. Add the folder where your agents keep them.
        </p>
      ) : (
        <ul aria-label="Agents found" className="flex flex-col gap-(--space-sm)">
          {agents.map((agent) => (
            <li
              key={agent.agent}
              className="flex items-center justify-between gap-(--space-md) rounded-control bg-surface-card px-(--space-md) py-(--space-sm)"
            >
              <span className="type-body font-semibold text-label">
                {agentDisplayName(agent.agent)}
              </span>
              <span className="type-callout tabular-nums text-label-secondary">
                {agent.sessionsSeen} {agent.sessionsSeen === 1 ? "session" : "sessions"}
              </span>
            </li>
          ))}
        </ul>
      )}
      <div>
        <PushButton onClick={() => void openSettingsWindow("sources")}>
          <FolderPlus size={12} aria-hidden="true" />
          Add a folder…
        </PushButton>
      </div>
    </div>
  )
}

export function useChecks(state: BurnChecksSnapshot) {
  const snoozes = useSnoozedBurnChecks()
  if (!state.report || snoozes.status !== "ready") return null
  return {
    presentation: checksPresentation(state.report, false, snoozedDetectorIds(snoozes.records)),
    snoozes: snoozes.records,
  }
}

function ScanTile({
  check,
  state,
  snoozed,
}: {
  check: ChecksCategoryPayload
  state: BurnChecksSnapshot
  snoozed: boolean
}) {
  const row = checkRowPresentation(check, state.targets[check.id]?.data?.targets)
  const summary = snoozed
    ? "Snoozed"
    : check.lifecycle == null
      ? "Not checked yet"
      : row.summary
  const failing = !snoozed && check.lifecycle === "failing"
  const total = check.finding + check.clean
  // Ten dots, like the HUD meters. A failing check lights at least one.
  const lit = failing && total > 0 ? Math.max(1, Math.round((check.finding / total) * 10)) : 0
  return (
    <li className="flex items-center gap-(--space-lg) rounded-(--radius-popover) bg-surface-card px-(--space-xl) py-(--space-lg)">
      <span
        aria-hidden="true"
        className={cn(
          "grid size-11 shrink-0 place-items-center rounded-full",
          snoozed ? "bg-surface-card text-label-secondary" : row.iconTone,
        )}
      >
        <row.Icon size={21} />
      </span>
      <span className="flex min-w-0 flex-1 flex-col gap-(--space-xs)">
        <span className="type-title-2 font-semibold text-label">{row.label}</span>
        <span className="flex items-center gap-(--space-md)">
          {failing && (
            <span aria-hidden="true" className="flex gap-[3px]">
              {Array.from({ length: 10 }, (_, index) => (
                <span
                  key={index}
                  className={cn(
                    "size-1.5 rounded-full",
                    index < lit ? "bg-brand-tint" : "bg-separator",
                  )}
                />
              ))}
            </span>
          )}
          <span className="type-title-3 font-normal! text-label-secondary tabular-nums">
            {summary}
          </span>
        </span>
      </span>
      {!snoozed && row.metric && (
        <span
          className={cn(
            "shrink-0 font-mono type-title-2 font-semibold tabular-nums",
            row.metricTone,
          )}
        >
          {row.metric}
        </span>
      )}
    </li>
  )
}

export function ScanStep({ state }: { state: BurnChecksSnapshot }) {
  const checks = useChecks(state)
  if (!checks) return <Waiting>Running burn checks…</Waiting>
  const { presentation } = checks
  const found = presentation.failures.length
  const ordered = [
    ...presentation.failures,
    ...(presentation.awaiting ?? []),
    ...presentation.wins,
    ...presentation.snoozed,
    ...presentation.unavailable,
  ]
  const snoozedIds = new Set(presentation.snoozed.map((check) => check.id))
  return (
    <div className="flex flex-col gap-(--space-lg)">
      <p className="type-body text-label">
        <span className="font-semibold">
          {found === 0 ? "Nothing to fix" : `${found} ${found === 1 ? "fix" : "fixes"} found`}
        </span>
        {found > 0 && ", biggest first on the next step."}
      </p>
      <ul aria-label="Burn checks" className="flex flex-col gap-(--space-sm)">
        {ordered.map((check) => (
          <ScanTile
            key={check.id}
            check={check}
            state={state}
            snoozed={snoozedIds.has(check.id)}
          />
        ))}
      </ul>
    </div>
  )
}

function FixCard({
  check,
  session,
  state,
}: {
  check: ChecksCategoryPayload
  session: BurnChecksSession
  state: BurnChecksSnapshot
}) {
  // The card asks for its targets while it is on screen, the same way the
  // Checks detail does.
  const trackTargets = useCallback(
    (node: HTMLElement | null) => session.setTargetsVisible(check.id, node !== null),
    [check.id, session],
  )
  const targets = state.targets[check.id]
  const row = checkRowPresentation(check, targets?.data?.targets)
  return (
    <article
      ref={trackTargets}
      aria-label={row.label}
      className="flex items-center gap-(--space-xl) rounded-(--radius-popover) bg-surface-sidebar p-(--space-lg) shadow-[var(--shadow-stats-card)]"
    >
      <div className="flex min-w-0 flex-1 flex-col gap-(--space-sm)">
        <header className="flex items-start gap-(--space-sm)">
          <span
            aria-hidden="true"
            className={cn("grid size-7 shrink-0 place-items-center rounded-full", row.iconTone)}
          >
            <row.Icon size={14} />
          </span>
          <div className="flex min-w-0 flex-1 flex-col">
            <h3 className="type-headline text-label">{row.label}</h3>
            <p className="type-caption tabular-nums text-label-secondary">
              {[row.summary, row.metric, row.costLine].filter(Boolean).join(" · ")}
            </p>
          </div>
        </header>
        <p className="type-body text-pretty text-label">{CHECK_SENTENCES[check.id]}</p>
        <p className="type-callout text-pretty text-label-secondary">
          {CHECK_UI[check.id].recommendation}
        </p>
      </div>
      <div className="shrink-0">
        {targets?.data ? (
          <CheckDetailActions
            detector={check.id}
            targets={targets.data.targets}
            refresh={session.refresh}
            reportRow
          />
        ) : (
          <div className="flex flex-wrap items-center gap-2">
            <RemindLaterAction detector={check.id} />
            <span role="status" className="type-callout text-label-secondary">
              {targets?.error ? "Could not load the fixes." : "Loading the fixes…"}
            </span>
          </div>
        )}
      </div>
    </article>
  )
}

export function FixStep({
  session,
  state,
}: {
  session: BurnChecksSession
  state: BurnChecksSnapshot
}) {
  const checks = useChecks(state)
  if (!checks) return <Waiting>Running burn checks…</Waiting>
  const { presentation, snoozes } = checks
  return (
    <div className="flex flex-col gap-(--space-lg)">
      {presentation.failures.length === 0 ? (
        <p className="type-body text-label-secondary">
          Nothing to fix. Every check passed, or you snoozed it.
        </p>
      ) : (
        presentation.failures.map((check) => (
          <FixCard key={check.id} check={check} session={session} state={state} />
        ))
      )}
      {presentation.snoozed.length > 0 && (
        <section aria-label="Snoozed checks" className="flex flex-col gap-(--space-sm)">
          <h3 className="type-callout font-semibold text-label-secondary">Snoozed</h3>
          {presentation.snoozed.map((check) => {
            const until = snoozes.find((record) => record.detector === check.id)?.until ?? null
            return (
              <div
                key={check.id}
                className="flex items-center justify-between gap-(--space-md) rounded-control bg-surface-card px-(--space-md) py-(--space-sm)"
              >
                <span className="min-w-0 type-callout text-label">
                  {checkRowPresentation(check).label}
                  <span className="text-label-secondary"> · {formatSnoozeUntil(until)}</span>
                </span>
                <RemindLaterAction detector={check.id} />
              </div>
            )
          })}
        </section>
      )}
    </div>
  )
}
