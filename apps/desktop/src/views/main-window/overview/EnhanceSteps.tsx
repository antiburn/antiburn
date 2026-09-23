import { FolderPlus } from "lucide-react"
import { useCallback, useState, useSyncExternalStore, type ReactNode, type Ref } from "react"

import { cn } from "../../../lib/cn"
import type { ChecksCategoryPayload } from "../../../lib/insightsIpc"
import { getScanStatus, openSettingsWindow } from "../../../lib/ipc"
import { renderAgentIcon } from "../../../lib/agentIcon"
import { agentDisplayName } from "../../../lib/presentation/agents"
import { checksPresentation, formatTokenBurnPercent } from "../../../lib/presentation/checks"
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

export type EnhanceTone = "fail" | "pass" | "wait" | "idle"

/** The one card every Enhance step uses: an icon on a solid chip, a title,
 *  a detail line, an optional body, and an optional aside on the right edge. */
export function EnhanceCard({
  as: Element = "li",
  cardRef,
  label,
  tone,
  icon,
  title,
  detail,
  aside,
  children,
}: {
  as?: "li" | "article"
  cardRef?: Ref<HTMLElement>
  label?: string
  tone: EnhanceTone
  icon: ReactNode
  title: ReactNode
  detail?: ReactNode
  aside?: ReactNode
  children?: ReactNode
}) {
  return (
    <Element
      ref={cardRef as Ref<HTMLLIElement & HTMLElement>}
      aria-label={label}
      data-tone={tone}
      className="enhance-card flex items-center gap-(--space-xl) rounded-(--radius-popover) px-(--space-xl) py-(--space-lg)"
    >
      <span
        aria-hidden="true"
        className="enhance-card-icon grid size-11 shrink-0 place-items-center rounded-(--radius-popover)"
      >
        {icon}
      </span>
      <div className="flex min-w-0 flex-1 flex-col gap-(--space-xs)">
        <span className="type-callout font-semibold text-label">{title}</span>
        {detail && (
          <span className="flex items-center gap-(--space-md) type-caption text-label-secondary tabular-nums">
            {detail}
          </span>
        )}
        {children}
      </div>
      {aside && <div className="shrink-0">{aside}</div>}
    </Element>
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
    <div className="flex flex-col gap-(--space-lg)">
      {agents.length === 0 ? (
        <p className="type-body text-label-secondary">
          No agent sessions found yet. Add the folder where your agents keep them.
        </p>
      ) : (
        <ul aria-label="Agents found" className="flex flex-col gap-(--space-sm)">
          {agents.map((agent) => (
            <EnhanceCard
              key={agent.agent}
              tone="idle"
              icon={renderAgentIcon(agent.agent, 24)}
              title={agentDisplayName(agent.agent)}
              detail={`${agent.sessionsSeen} ${agent.sessionsSeen === 1 ? "session" : "sessions"}`}
            />
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
    <EnhanceCard
      tone={snoozed || check.lifecycle == null ? "idle" : failing ? "fail" : "pass"}
      icon={<row.Icon size={22} strokeWidth={2} />}
      title={row.label}
      detail={
        <>
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
          {summary}
        </>
      }
      aside={
        !snoozed &&
        check.estimatedTokenBurnBasisPoints != null &&
        row.metric && (
          <span className="enhance-card-metric flex flex-col items-end text-end">
            <span className="type-title-1 font-semibold tabular-nums">
              {formatTokenBurnPercent(check.estimatedTokenBurnBasisPoints)}
            </span>
            <span className="type-caption text-label-secondary">estimated burn</span>
          </span>
        )
      }
    />
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
    <EnhanceCard
      as="article"
      cardRef={trackTargets}
      label={row.label}
      tone="fail"
      icon={<row.Icon size={22} strokeWidth={2} />}
      title={row.label}
      detail={[row.summary, row.metric, row.costLine].filter(Boolean).join(" · ")}
      aside={
        targets?.data ? (
          <div className="enhance-card-actions">
            <CheckDetailActions
              detector={check.id}
              targets={targets.data.targets}
              refresh={session.refresh}
              reportRow
            />
          </div>
        ) : (
          <div className="flex flex-wrap items-center gap-2">
            <RemindLaterAction detector={check.id} />
            <span role="status" className="type-callout text-label-secondary">
              {targets?.error ? "Could not load the fixes." : "Loading the fixes…"}
            </span>
          </div>
        )
      }
    >
      <p className="mt-(--space-xs) type-body text-pretty text-label">
        {CHECK_SENTENCES[check.id]}
      </p>
      <p className="type-callout text-pretty text-label-secondary">
        {CHECK_UI[check.id].recommendation}
      </p>
    </EnhanceCard>
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
