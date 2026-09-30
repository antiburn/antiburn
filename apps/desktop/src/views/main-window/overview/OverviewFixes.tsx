import { useRef, useSyncExternalStore } from "react"
import { flushSync } from "react-dom"
import { CircleAlert, CircleCheck } from "lucide-react"

import { cn } from "../../../lib/cn"
import {
  dismissFtueCallout,
  FTUE_SESSION_TOTAL,
  ftueSnapshot,
  subscribeFtue,
} from "./ftuePrototype"

// Prototype data. The rows stand in for the config checks the product runs.
const CHECKS: ReadonlyArray<{ title: string; detail: string; needsFix: boolean }> = [
  {
    title: "Thinking level",
    detail: "Sessions think at high on simple edits",
    needsFix: true,
  },
  {
    title: "Subagent config",
    detail: "Subagents run on the main model",
    needsFix: true,
  },
  {
    title: "Autocompact settings",
    detail: "Context runs past 400k before compaction",
    needsFix: true,
  },
  {
    title: "Unused MCP servers",
    detail: "2 servers loaded, never called",
    needsFix: true,
  },
  {
    title: "Unused skills",
    detail: "3 skills injected, never used",
    needsFix: true,
  },
  { title: "Built-in tools", detail: "All loaded tools in use", needsFix: false },
  { title: "Model versions", detail: "No old models in use", needsFix: false },
  { title: "Fast mode", detail: "Used where it pays off", needsFix: false },
  { title: "Cache churn", detail: "Cache hit rate is healthy", needsFix: false },
]

const FIX_COUNT = CHECKS.filter((check) => check.needsFix).length

const FLY_DURATION_MS = 550

export function OverviewFixes() {
  // Prototype: the state lives in memory only, so a reload brings the
  // callout back.
  const { reading, analysis, dismissed } = useSyncExternalStore(
    subscribeFtue,
    ftueSnapshot,
    ftueSnapshot,
  )
  const scanned = reading >= 1 && analysis >= 1
  const ctaRef = useRef<HTMLButtonElement | null>(null)
  const cornerRef = useRef<HTMLButtonElement | null>(null)

  function dismiss(): void {
    const from = ctaRef.current?.getBoundingClientRect()
    flushSync(dismissFtueCallout)
    const target = cornerRef.current
    if (!from || !target) return
    if (window.matchMedia("(prefers-reduced-motion: reduce)").matches) return
    const to = target.getBoundingClientRect()
    const dx = from.left + from.width / 2 - (to.left + to.width / 2)
    const dy = from.top + from.height / 2 - (to.top + to.height / 2)
    target.animate(
      [
        {
          transform: `translate(${dx}px, ${dy}px) scale(${from.width / to.width}, ${from.height / to.height})`,
        },
        { transform: "none" },
      ],
      { duration: FLY_DURATION_MS, easing: "cubic-bezier(0.2, 0.8, 0.2, 1)" },
    )
  }

  return (
    <section aria-label="Fixes" className="relative min-h-[220px] flex-1">
      {/* Out of flow, so the list takes the space the page leaves it and
          never grows the page. It clips and fades out at the bottom. */}
      <div
        className={cn(
          "absolute inset-0 flex flex-col overflow-hidden mask-b-from-75%",
          // Prototype: the checks wait two seconds after the callout, then
          // fade up slowly. A reset hides them at once.
          scanned
            ? "transition-opacity [transition-delay:2000ms] [transition-duration:1500ms]"
            : "opacity-0",
        )}
      >
        <h2 className="mb-(--space-sm) type-caption text-label-secondary">Config checks</h2>

        <ul className="flex flex-col gap-1">
          {CHECKS.map((check) => (
            <li
              key={check.title}
              className={cn(
                "flex items-center gap-3 rounded-(--radius-popover) px-3 py-1.5",
                check.needsFix
                  ? "bg-brand-tint/12 ring-1 ring-brand-tint/40"
                  : "bg-session-card",
              )}
            >
              {check.needsFix ? (
                <CircleAlert size={16} strokeWidth={2} className="shrink-0 text-brand" />
              ) : (
                <CircleCheck
                  size={16}
                  strokeWidth={2}
                  className="shrink-0 text-label-tertiary"
                />
              )}
              <span className="flex min-w-0 items-baseline gap-2">
                <span
                  className={cn(
                    "shrink-0 type-body font-medium!",
                    check.needsFix ? "text-label" : "text-label-secondary",
                  )}
                >
                  {check.title}
                </span>
                <span className="truncate type-footnote text-label-tertiary">
                  {check.detail}
                </span>
              </span>
              <span
                className={cn(
                  "ms-auto shrink-0 font-mono type-metadata",
                  check.needsFix ? "text-brand" : "text-label-tertiary",
                )}
              >
                {check.needsFix ? "needs fix" : "passing"}
              </span>
            </li>
          ))}
        </ul>
      </div>

      <div
        inert={dismissed}
        className={cn(
          "absolute inset-0 flex flex-col items-center justify-center gap-(--space-lg) bg-surface-window/75 p-(--space-lg) text-center rounded-(--radius-popover) backdrop-blur-[1.5px] transition-opacity duration-slow",
          dismissed && "pointer-events-none opacity-0",
        )}
      >
        <div className="flex w-full max-w-[26rem] flex-col gap-(--space-md) text-start">
          <ScanBar
            label="Reading sessions"
            value={reading}
            detail={`${Math.round(reading * FTUE_SESSION_TOTAL)} of ${FTUE_SESSION_TOTAL}`}
          />
          <ScanBar
            label="Analysing"
            value={analysis}
            detail={reading < 1 ? "waiting" : `${Math.round(analysis * 100)}%`}
          />
        </div>

        {/* Holds its space while the scan runs, so the bars do not move when
            it arrives. */}
        <div
          inert={!scanned}
          className={cn("flex flex-col items-center gap-(--space-lg)", !scanned && "opacity-0")}
        >
          <div className="mt-6 flex max-w-[36rem] flex-col gap-(--space-xs)">
            <p className="type-title-1 font-semibold! text-label">
              {FIX_COUNT} fixes found in your config
            </p>
            <p className="type-body text-label-secondary">
              Thinking level needs to be reduced, subagent config could be improved, autocompact
              settings not optimal, +{FIX_COUNT - 3} more
            </p>
          </div>
          <div
            className={cn(
              "flex flex-col items-center gap-(--space-sm)",
              dismissed && "invisible",
            )}
          >
            <button
              ref={ctaRef}
              type="button"
              className="rounded-control bg-brand-tint px-8 py-2.5 type-headline font-semibold! text-white shadow-[var(--shadow-raised)] transition-[filter] duration-fast hover:brightness-110 active:brightness-95"
            >
              Enhance
            </button>
            <button
              type="button"
              onClick={dismiss}
              className="type-footnote text-label-secondary underline underline-offset-[3px] hover:text-label"
            >
              Dismiss
            </button>
          </div>
        </div>
      </div>

      {dismissed && (
        <button
          ref={cornerRef}
          type="button"
          className="absolute right-0 bottom-0 origin-center rounded-control bg-brand-tint px-4 py-1.5 type-callout font-semibold! text-white shadow-[var(--shadow-raised)] transition-[filter] duration-fast hover:brightness-110 active:brightness-95"
        >
          Enhance
        </button>
      )}
    </section>
  )
}

function ScanBar({ label, value, detail }: { label: string; value: number; detail: string }) {
  const done = value >= 1
  return (
    <div className="flex flex-col gap-1.5">
      <div className="flex items-baseline justify-between">
        <span className={cn("type-callout", done ? "text-label-secondary" : "text-label")}>
          {label}
        </span>
        <span className="font-mono type-metadata tabular-nums text-label-tertiary">
          {detail}
        </span>
      </div>
      <div
        role="progressbar"
        aria-label={label}
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={Math.round(value * 100)}
        className="h-1.5 overflow-hidden rounded-full bg-surface-tertiary"
      >
        <div
          className="h-full rounded-full bg-brand-tint transition-[width] duration-medium ease-out"
          style={{ width: `${value * 100}%` }}
        />
      </div>
    </div>
  )
}
