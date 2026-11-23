import * as DropdownMenu from "@radix-ui/react-dropdown-menu"
import * as TooltipPrimitive from "@radix-ui/react-tooltip"
import { Check, ListFilter, Minus, X } from "lucide-react"
import { useCallback, useRef, useState, type MouseEvent, type RefObject } from "react"

import { pluralize } from "../../lib/presentation/text"
import { renderAgentIcon } from "../../lib/agentIcon"
import { isMacOS } from "../../lib/platform"
import { agentDisplayName } from "../../lib/presentation/agents"
import {
  BURN_CHECK_MARKS,
  type BurnCheckMark,
} from "../../components/burn-checks/burnCheckMarks"
import type {
  SessionFilterCounts,
  SessionFilters,
  SessionResultFilter,
  SessionSpendFilter,
} from "../../lib/sessionFilters"
import { Tooltip } from "../../components/presentation/Tooltip"
import { CollectionHeader } from "../../components/ui/CollectionHeader"
import { CountPill } from "../../components/ui/CountPill"
import { observeMenuPointerExit } from "../../components/ui/menuPointerDismissal"

export interface SessionFiltersHeaderProps {
  filters: SessionFilters
  counts: SessionFilterCounts
  /** Indexed sessions in the date range before the viewer applies collection filters. */
  sessionTotal?: number | null
  /** True while the stream can supply more rows for these counts. */
  hasMoreSessions?: boolean
  /** True while loaded sessions still need results for the selected filter. */
  awaitingResultFilter?: boolean
  agents: string[]
  onToggleAgent: (agent: string) => void
  onResetAgents: () => void
  onResultChange: (result: SessionResultFilter) => void
  onSpendChange: (spend: SessionSpendFilter) => void
  onClear: () => void
  highCostThresholdUsd?: number | undefined
  days: number
  onChangeTimeRange: () => void
  triggerRef?: RefObject<HTMLButtonElement | null>
}

type ActiveChip = {
  id: string
  label: string
  accessibleLabel?: string
  agent?: string
  mark?: BurnCheckMark
  tooltip?: string | undefined
  remove: () => void
}

type FilterOption<T> = {
  value: T
  label: string
  accessibleLabel?: string
  description?: string | undefined
  tooltip?: string | undefined
  mark?: BurnCheckMark
}

const RESULT_OPTIONS: ReadonlyArray<FilterOption<SessionResultFilter>> = [
  { value: "all", label: "All results" },
  { value: "failing", label: "Failed", mark: BURN_CHECK_MARKS.finding },
  { value: "passing", label: "Passed", mark: BURN_CHECK_MARKS.clean },
]

const HIGH_COST_EXPLANATION =
  "Based on all agents in this time range: 3× the median session cost, with a $2 minimum."

function sessionCountLabel(count: number): string {
  return `${count} matching ${pluralize(count, "session", "sessions")}`
}

function VendorIcon({ agent, compact = false }: { agent: string; compact?: boolean }) {
  return (
    <span
      className="inline-flex size-3.5 shrink-0 items-center justify-center"
      data-session-filter-agent={agent}
    >
      {renderAgentIcon(agent, compact ? 12 : 14)}
    </span>
  )
}

function Indicator({
  checkbox = false,
  mixed = false,
}: {
  checkbox?: boolean
  mixed?: boolean
}) {
  return (
    <span
      className={
        checkbox
          ? "flex size-4 shrink-0 items-center justify-center rounded-control border border-separator"
          : "flex size-4 shrink-0 items-center justify-center"
      }
    >
      {mixed ? (
        <Minus size={12} aria-hidden="true" />
      ) : (
        <DropdownMenu.ItemIndicator>
          <Check size={12} aria-hidden="true" />
        </DropdownMenu.ItemIndicator>
      )}
    </span>
  )
}

function MenuCount({ count }: { count: number }) {
  return <CountPill count={count} className="session-filter-count ml-auto" aria-hidden="true" />
}

function FilterRadioGroup<T extends SessionResultFilter | SessionSpendFilter>({
  label,
  value,
  options,
  counts,
  hasMoreSessions,
  onChange,
}: {
  label: string
  value: T
  options: ReadonlyArray<FilterOption<T>>
  counts: Record<T, number>
  hasMoreSessions: boolean
  onChange: (value: T) => void
}) {
  return (
    <DropdownMenu.Group>
      <DropdownMenu.Label className="px-2 pt-2 pb-1 type-caption text-label-secondary">
        {label}
      </DropdownMenu.Label>
      <DropdownMenu.RadioGroup value={value} onValueChange={(next) => onChange(next as T)}>
        {options.map((option) => {
          const Mark = option.mark?.Icon
          const item = (
            <DropdownMenu.RadioItem
              key={option.value}
              value={option.value}
              disabled={
                !hasMoreSessions &&
                counts[option.value] === 0 &&
                option.value !== value &&
                option.value !== "all"
              }
              textValue={option.accessibleLabel ?? option.label}
              aria-label={`${option.accessibleLabel ?? option.label}${option.description ? `, ${option.description}` : ""}, ${sessionCountLabel(counts[option.value])}`}
              className="ui-menu-item"
              onSelect={(event) => event.preventDefault()}
            >
              <Indicator />
              {Mark ? (
                <Mark
                  size={14}
                  strokeWidth={option.mark?.strokeWidth}
                  className={`session-filter-result-mark shrink-0 ${option.mark?.iconClass}`}
                  aria-hidden="true"
                />
              ) : null}
              <span className="flex flex-col">
                <span>{option.label}</span>
                {option.description ? (
                  <span className="session-filter-description type-caption text-label-secondary">
                    {option.description}
                  </span>
                ) : null}
              </span>
              <MenuCount count={counts[option.value]} />
            </DropdownMenu.RadioItem>
          )
          return option.tooltip ? (
            <Tooltip key={option.value} label={option.tooltip} side="left">
              {item}
            </Tooltip>
          ) : (
            item
          )
        })}
      </DropdownMenu.RadioGroup>
    </DropdownMenu.Group>
  )
}

function FilterChip({
  chip,
  assignRef,
  onRemove,
}: {
  chip: ActiveChip
  assignRef: (node: HTMLButtonElement | null) => void
  onRemove: (event: MouseEvent<HTMLButtonElement>) => void
}) {
  const Mark = chip.mark?.Icon
  const button = (
    <button
      ref={assignRef}
      type="button"
      className="session-filter-target relative inline-flex items-center gap-1 whitespace-nowrap rounded-control bg-surface-card px-2 py-1 type-caption text-label-secondary hover:bg-surface-secondary hover:text-label"
      aria-label={`Remove ${chip.accessibleLabel ?? chip.label} filter`}
      onClick={onRemove}
    >
      {Mark ? (
        <Mark
          size={14}
          strokeWidth={chip.mark?.strokeWidth}
          className={`me-1 shrink-0 ${chip.mark?.iconClass}`}
          aria-hidden="true"
        />
      ) : null}
      {chip.agent ? (
        <VendorIcon agent={chip.agent} compact />
      ) : (
        <span className="text-label">{chip.label}</span>
      )}
      <X size={10} aria-hidden="true" />
    </button>
  )
  return chip.tooltip ? <Tooltip label={chip.tooltip}>{button}</Tooltip> : button
}

/** Contextual filters and count for the Sessions collection. */
export function SessionFiltersHeader({
  filters,
  counts,
  sessionTotal = null,
  hasMoreSessions = false,
  awaitingResultFilter = false,
  agents,
  onToggleAgent,
  onResetAgents,
  onResultChange,
  onSpendChange,
  onClear,
  highCostThresholdUsd,
  days,
  onChangeTimeRange,
  triggerRef,
}: SessionFiltersHeaderProps) {
  const localTriggerRef = useRef<HTMLButtonElement | null>(null)
  const filterButtonRef = triggerRef ?? localTriggerRef
  const chipRefs = useRef(new Map<string, HTMLButtonElement>())
  const [menuOpen, setMenuOpen] = useState(false)
  const [filterTooltipOpen, setFilterTooltipOpen] = useState(false)
  const pointerOpened = useRef(false)
  const pointerDismissed = useRef(false)
  const observeMenu = useCallback(
    (content: HTMLDivElement | null) => {
      if (!content) return
      return observeMenuPointerExit({
        content,
        trigger: () => filterButtonRef.current,
        pointerOpened: pointerOpened.current,
        onDismiss: () => {
          pointerDismissed.current = true
          setMenuOpen(false)
        },
      })
    },
    [filterButtonRef],
  )
  const highCostExplanation = hasMoreSessions
    ? "Based on loaded sessions: 3× the median session cost, with a $2 minimum."
    : HIGH_COST_EXPLANATION
  const highCostDescription =
    highCostThresholdUsd === undefined
      ? undefined
      : `Over $${highCostThresholdUsd.toFixed(2).replace(/\.00$/, "")}`
  const spendOptions: ReadonlyArray<FilterOption<SessionSpendFilter>> = [
    { value: "all", label: "All costs" },
    {
      value: "notable",
      label: "High cost",
      description: highCostDescription,
      tooltip: highCostExplanation,
    },
    { value: "material", label: "$1 or more" },
  ]
  const chips: ActiveChip[] = [
    ...filters.agents.map((agent) => ({
      id: `agent:${agent}`,
      label: agentDisplayName(agent),
      tooltip: agentDisplayName(agent),
      agent,
      remove: () => onToggleAgent(agent),
    })),
    ...(filters.result === "all"
      ? []
      : [
          {
            id: `result:${filters.result}`,
            label: filters.result === "failing" ? "Failed" : "Passed",
            mark:
              filters.result === "failing" ? BURN_CHECK_MARKS.finding : BURN_CHECK_MARKS.clean,
            remove: () => onResultChange("all"),
          },
        ]),
    ...(filters.spend === "all"
      ? []
      : [
          {
            id: `spend:${filters.spend}`,
            label: filters.spend === "notable" ? "High cost" : "≥ $1",
            accessibleLabel: filters.spend === "notable" ? "High cost" : "$1 or more",
            tooltip:
              filters.spend === "notable"
                ? [highCostDescription, highCostExplanation].filter(Boolean).join(". ")
                : undefined,
            remove: () => onSpendChange("all"),
          },
        ]),
  ]

  function restoreFocus(targetId?: string) {
    queueMicrotask(() => {
      if (targetId) chipRefs.current.get(targetId)?.focus()
      else filterButtonRef.current?.focus()
    })
  }

  function removeChip(chip: ActiveChip, event: MouseEvent<HTMLButtonElement>) {
    const index = chips.findIndex((candidate) => candidate.id === chip.id)
    const focusTarget = chips[index + 1]?.id ?? chips[index - 1]?.id
    chip.remove()
    if (event.detail === 0) restoreFocus(focusTarget)
  }

  const hasFilters =
    filters.agents.length > 0 || filters.result !== "all" || filters.spend !== "all"
  const totalIsPartial = sessionTotal === null && hasMoreSessions
  const displayTotal = sessionTotal ?? counts.all
  const matchesArePartial = hasMoreSessions || awaitingResultFilter
  const matchLabel = sessionCountLabel(counts.matching)

  return (
    <CollectionHeader
      title="Sessions"
      dragRegion={isMacOS()}
      summary={
        <>
          <Tooltip
            label={
              totalIsPartial
                ? "Sessions loaded so far, before list filters."
                : "All sessions in the selected date range, before list filters."
            }
          >
            <span tabIndex={0} className="inline-flex shrink-0 rounded-full">
              <CountPill count={displayTotal} size="regular" aria-hidden="true" />
              <span className="sr-only">
                {totalIsPartial
                  ? `${displayTotal} sessions loaded`
                  : `${displayTotal} sessions in selected date range`}
              </span>
            </span>
          </Tooltip>
          {hasFilters && (
            <Tooltip
              label={
                matchesArePartial
                  ? "Matches among loaded sessions. The count can change as more sessions load or checks finish."
                  : "Sessions in the selected date range that match all active filters."
              }
            >
              <span
                tabIndex={0}
                className="whitespace-nowrap rounded-control type-caption tabular-nums text-label-secondary"
              >
                <span className="me-2 text-label-tertiary" aria-hidden="true">
                  ·
                </span>
                <span aria-hidden="true">Showing {counts.matching}</span>
                <span className="sr-only">
                  {matchesArePartial ? `${matchLabel} among loaded sessions` : matchLabel}
                </span>
              </span>
            </Tooltip>
          )}
          <span className="sr-only" aria-live="polite" aria-atomic="true">
            {`${counts.all} ${hasMoreSessions ? "loaded" : "total"} sessions, ${counts.matching} matching`}
          </span>
        </>
      }
      actions={
        <div className="session-filter-actions flex shrink-0 items-center gap-2">
          <Tooltip label="Change time range in Settings → Sessions. Active sessions are always included.">
            <button
              type="button"
              className="session-filter-target relative inline-flex shrink-0 items-center whitespace-nowrap rounded-control px-1 py-1 type-caption text-label-secondary hover:bg-surface-hover hover:text-label"
              aria-label={`${days === 1 ? "Today" : `Last ${days} days`}, change time range in Settings → Sessions`}
              onClick={onChangeTimeRange}
            >
              {days === 1 ? "Today" : `${days} days`}
            </button>
          </Tooltip>
          <DropdownMenu.Root
            modal={false}
            open={menuOpen}
            onOpenChange={(open) => {
              pointerDismissed.current = false
              setFilterTooltipOpen(false)
              setMenuOpen(open)
            }}
          >
            <TooltipPrimitive.Provider delayDuration={600}>
              <TooltipPrimitive.Root
                open={filterTooltipOpen && !menuOpen}
                onOpenChange={(open) => setFilterTooltipOpen(open && !menuOpen)}
              >
                <DropdownMenu.Trigger asChild>
                  <TooltipPrimitive.Trigger asChild>
                    <button
                      ref={filterButtonRef}
                      type="button"
                      onPointerDown={(event) => {
                        pointerOpened.current = event.pointerType === "mouse"
                      }}
                      onKeyDown={() => {
                        pointerOpened.current = false
                      }}
                      aria-label="Filters"
                      className="session-filter-target relative inline-flex h-[var(--control-height-regular)] w-[var(--control-height-regular)] shrink-0 items-center justify-center rounded-full text-label-secondary hover:bg-surface-hover hover:text-label data-[state=open]:bg-surface-selected data-[state=open]:text-label"
                    >
                      <ListFilter size={14} aria-hidden="true" />
                    </button>
                  </TooltipPrimitive.Trigger>
                </DropdownMenu.Trigger>
                {!menuOpen && (
                  <TooltipPrimitive.Portal>
                    <TooltipPrimitive.Content
                      side="bottom"
                      sideOffset={4}
                      collisionPadding={8}
                      className="ui-tooltip max-w-[220px] whitespace-normal"
                    >
                      Filter sessions
                    </TooltipPrimitive.Content>
                  </TooltipPrimitive.Portal>
                )}
              </TooltipPrimitive.Root>
            </TooltipPrimitive.Provider>
            <DropdownMenu.Portal>
              <DropdownMenu.Content
                ref={observeMenu}
                onCloseAutoFocus={(event) => {
                  if (pointerDismissed.current) event.preventDefault()
                  pointerDismissed.current = false
                }}
                className="ui-menu session-filters-menu min-w-56"
                side="bottom"
                align="end"
                sideOffset={4}
                collisionPadding={8}
              >
                <DropdownMenu.Group>
                  <DropdownMenu.Label className="px-2 pt-2 pb-1 type-caption text-label-secondary">
                    Agents · Select one or more
                  </DropdownMenu.Label>
                  <DropdownMenu.CheckboxItem
                    checked={filters.agents.length === 0}
                    className="ui-menu-item"
                    textValue="All agents"
                    aria-label={`All agents, ${sessionCountLabel(counts.agentsAll)}`}
                    onSelect={(event) => event.preventDefault()}
                    onCheckedChange={onResetAgents}
                  >
                    <Indicator checkbox />
                    <span>All agents</span>
                    <MenuCount count={counts.agentsAll} />
                  </DropdownMenu.CheckboxItem>
                  {agents.map((agent) => {
                    const label = agentDisplayName(agent)
                    const count = counts.agents[agent] ?? 0
                    return (
                      <DropdownMenu.CheckboxItem
                        key={agent}
                        checked={filters.agents.includes(agent)}
                        disabled={
                          !hasMoreSessions && count === 0 && !filters.agents.includes(agent)
                        }
                        className="ui-menu-item"
                        textValue={label}
                        aria-label={`${label}, ${sessionCountLabel(count)}`}
                        onSelect={(event) => event.preventDefault()}
                        onCheckedChange={() => onToggleAgent(agent)}
                      >
                        <Indicator checkbox />
                        <VendorIcon agent={agent} />
                        <span>{label}</span>
                        <MenuCount count={count} />
                      </DropdownMenu.CheckboxItem>
                    )
                  })}
                </DropdownMenu.Group>
                <DropdownMenu.Separator className="ui-menu-separator" />
                <FilterRadioGroup
                  label="Check result"
                  value={filters.result}
                  options={RESULT_OPTIONS}
                  counts={counts.result}
                  hasMoreSessions={hasMoreSessions}
                  onChange={onResultChange}
                />
                <DropdownMenu.Separator className="ui-menu-separator" />
                <FilterRadioGroup
                  label="Spend"
                  value={filters.spend}
                  options={spendOptions}
                  counts={counts.spend}
                  hasMoreSessions={hasMoreSessions}
                  onChange={onSpendChange}
                />
                {chips.length > 0 ? (
                  <>
                    <DropdownMenu.Separator className="ui-menu-separator" />
                    <DropdownMenu.Item className="ui-menu-item" onSelect={onClear}>
                      <span className="size-4 shrink-0" aria-hidden="true" />
                      Clear filters
                    </DropdownMenu.Item>
                  </>
                ) : null}
              </DropdownMenu.Content>
            </DropdownMenu.Portal>
          </DropdownMenu.Root>
        </div>
      }
    >
      {chips.length > 0 ? (
        <div
          className="mt-2 flex min-w-0 flex-wrap items-center gap-2"
          data-active-session-filters=""
        >
          {chips.map((chip) => (
            <FilterChip
              key={chip.id}
              chip={chip}
              assignRef={(node) => {
                if (node) chipRefs.current.set(chip.id, node)
                else chipRefs.current.delete(chip.id)
              }}
              onRemove={(event) => removeChip(chip, event)}
            />
          ))}
        </div>
      ) : null}
    </CollectionHeader>
  )
}
