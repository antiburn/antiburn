// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { act, fireEvent, render, screen } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import type {
  LiveProviderUsagePayload,
  LiveUsageSummaryPayload,
  LiveUsageWindowPayload,
  ProviderUsagePayload,
  ProviderUsageWindowPayload,
} from "../../lib/ipc"
import { ProviderUsageChips } from "./ProviderUsageChips"

function usageWindow(
  overrides: Partial<ProviderUsageWindowPayload> = {},
): ProviderUsageWindowPayload {
  return {
    tokensIn: 0,
    tokensOut: 0,
    cacheRead: 0,
    estimatedUsd: null,
    sessionCount: 0,
    ...overrides,
  }
}

function provider(overrides: Partial<ProviderUsagePayload> = {}): ProviderUsagePayload {
  const today = usageWindow({ estimatedUsd: 1.25, tokensIn: 1_000, sessionCount: 1 })
  return {
    provider: "anthropic",
    displayName: "Anthropic",
    state: "estimated",
    staleness: "fresh",
    windows: { today, week: today, month: today },
    lastActivityAt: new Date().toISOString(),
    ...overrides,
  }
}

const FORECAST = {
  unavailableReason: "sparseHistory",
  confidence: null,
  consumptionRate: null,
  paceRatio: null,
  paceTrend: null,
  runwayAt: null,
  usedToday: null,
}

function liveWindow(overrides: Partial<LiveUsageWindowPayload> = {}): LiveUsageWindowPayload {
  return {
    id: "seven-day",
    role: "primaryLong",
    kind: "weekly",
    scopeModel: null,
    usedPercent: 42,
    startsAt: null,
    resetsAt: null,
    hasNonzeroUsageInCurrentPeriod: false,
    forecast: FORECAST,
    ...overrides,
  }
}

function liveProvider(
  overrides: Partial<LiveProviderUsagePayload> = {},
): LiveProviderUsagePayload {
  return {
    provider: "anthropic",
    displayName: "Anthropic",
    support: "live",
    freshness: "fresh",
    sourceLabel: "Asked Claude directly",
    observedAt: new Date().toISOString(),
    windows: [liveWindow()],
    extraUsage: null,
    ...overrides,
  }
}

function liveSummary(
  providers: readonly LiveProviderUsagePayload[] = [liveProvider()],
): LiveUsageSummaryPayload {
  return { providers: [...providers], errors: [], generatedAt: new Date().toISOString() }
}

/** Live providers ranked below the first, so the overflow affordance appears. */
function rankedLive(count: number): LiveProviderUsagePayload[] {
  return Array.from({ length: count }, (_, index) =>
    liveProvider({
      provider: `p${index}`,
      displayName: `Provider ${index}`,
      windows: [liveWindow({ usedPercent: count - index })],
    }),
  )
}

describe("ProviderUsageChips", () => {
  it("shows a chip from a live reading alone, even when nothing was spent today", () => {
    // The bug this fixes: a fresh day with no local spend used to fall
    // through to the empty-row fallback, even though the provider's own
    // limits were sitting right there in `live`.
    render(
      <ProviderUsageChips
        providers={[]}
        live={liveSummary([liveProvider({ displayName: "Claude" })])}
        onViewAll={vi.fn()}
      />,
    )

    expect(screen.getByRole("button", { name: /^Claude,/ })).toBeInTheDocument()
    expect(screen.queryByText("No live limits")).not.toBeInTheDocument()
  })

  it("drops the local-state clause when the provider has no local spend to describe", () => {
    render(
      <ProviderUsageChips
        providers={[]}
        live={liveSummary([liveProvider({ displayName: "Claude" })])}
        onViewAll={vi.fn()}
      />,
    )

    expect(screen.getByRole("button", { name: "Claude, weekly limit 42%" })).toBeInTheDocument()
  })

  it("carries the local state into the chip name when local spend exists alongside the live reading", () => {
    const window = usageWindow({ tokensIn: 12_000, sessionCount: 2 })
    render(
      <ProviderUsageChips
        providers={[
          provider({
            state: "observed",
            windows: { today: window, week: window, month: window },
          }),
        ]}
        live={liveSummary()}
        onViewAll={vi.fn()}
      />,
    )

    expect(
      screen.getByRole("button", { name: "Anthropic, observed, weekly limit 42%" }),
    ).toBeInTheDocument()
  })

  it("carries staleness into the chip name rather than only into a color", () => {
    render(
      <ProviderUsageChips
        providers={[
          provider({
            staleness: "stale",
            lastActivityAt: new Date(Date.now() - 2 * 86_400_000).toISOString(),
          }),
        ]}
        live={liveSummary()}
        onViewAll={vi.fn()}
      />,
    )

    expect(
      screen.getByRole("button", {
        name: /anthropic, estimated, weekly limit 42%, last used 2d ago/i,
      }),
    ).toBeInTheDocument()
  })

  it("says so honestly when no provider has a live limit", () => {
    render(<ProviderUsageChips providers={[provider()]} onViewAll={vi.fn()} />)

    // Local spend alone no longer earns a chip — see the selection doc.
    expect(screen.getByText("No live limits")).toBeInTheDocument()
    expect(screen.queryByRole("button", { name: /anthropic/i })).not.toBeInTheDocument()
  })

  it("collapses everything past the chip budget into one overflow affordance", () => {
    const onViewAll = vi.fn()
    render(
      <ProviderUsageChips
        providers={[]}
        live={liveSummary(rankedLive(5))}
        onViewAll={onViewAll}
        maxVisible={3}
      />,
    )

    expect(screen.getAllByRole("button", { name: /^Provider \d/ })).toHaveLength(3)
    const overflow = screen.getByRole("button", { name: "Show 2 more providers" })
    fireEvent.click(overflow)
    expect(onViewAll).toHaveBeenCalledTimes(1)
  })

  it("opens a provider panel on click and closes it on a second click", () => {
    render(
      <ProviderUsageChips providers={[provider()]} live={liveSummary()} onViewAll={vi.fn()} />,
    )
    const chip = screen.getByRole("button", { name: /anthropic/i })

    expect(chip).toHaveAttribute("aria-expanded", "false")
    fireEvent.click(chip)

    const panel = screen.getByRole("dialog", { name: "Anthropic" })
    expect(panel).toBeInTheDocument()
    expect(chip).toHaveAttribute("aria-expanded", "true")
    expect(chip).toHaveAttribute("aria-controls", panel.id)
    // The panel states what the figures are, not how much of an allowance is left.
    expect(screen.getByText(/priced on this device/i)).toBeInTheDocument()

    fireEvent.click(chip)
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument()
  })

  it("opens a panel with only the live limits, and no spend section, for a live-only provider", () => {
    render(
      <ProviderUsageChips
        providers={[]}
        live={liveSummary([liveProvider({ displayName: "Claude" })])}
        onViewAll={vi.fn()}
      />,
    )

    fireEvent.click(screen.getByRole("button", { name: /^Claude,/ }))
    const panel = screen.getByRole("dialog")
    expect(panel).toHaveTextContent("Plan limits")
    // Nothing was ever run through this provider on this device, so the
    // panel has no spend half to show — not even a zeroed one.
    expect(panel).not.toHaveTextContent("Tokens · this month")
    expect(panel).not.toHaveTextContent("Spend trend")
  })

  it("closes the panel on Escape and on a press outside it", () => {
    render(
      <ProviderUsageChips providers={[provider()]} live={liveSummary()} onViewAll={vi.fn()} />,
    )
    const chip = screen.getByRole("button", { name: /anthropic/i })

    fireEvent.click(chip)
    fireEvent.keyDown(document, { key: "Escape" })
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument()

    fireEvent.click(chip)
    expect(screen.getByRole("dialog")).toBeInTheDocument()
    fireEvent.pointerDown(document.body)
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument()
  })

  it("leads to the full view from the panel", () => {
    const onViewAll = vi.fn()
    render(
      <ProviderUsageChips
        providers={[provider()]}
        live={liveSummary()}
        onViewAll={onViewAll}
      />,
    )

    fireEvent.click(screen.getByRole("button", { name: /anthropic/i }))
    fireEvent.click(screen.getByRole("button", { name: "All provider usage" }))
    expect(onViewAll).toHaveBeenCalledTimes(1)
    // Navigating away also dismisses the panel, so returning shows the list.
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument()
  })

  it("moves focus into the panel it opens", () => {
    render(
      <ProviderUsageChips providers={[provider()]} live={liveSummary()} onViewAll={vi.fn()} />,
    )

    fireEvent.click(screen.getByRole("button", { name: /anthropic/i }))

    // `role="dialog"` promises containment. A reader who is not in the dialog
    // has been told about a surface they cannot reach.
    const panel = screen.getByRole("dialog")
    expect(panel.contains(document.activeElement)).toBe(true)
    expect(screen.getByRole("button", { name: "All provider usage" })).toHaveFocus()
  })

  it("holds Tab inside the panel while it is open", () => {
    render(
      <ProviderUsageChips providers={[provider()]} live={liveSummary()} onViewAll={vi.fn()} />,
    )

    fireEvent.click(screen.getByRole("button", { name: /anthropic/i }))
    const panel = screen.getByRole("dialog")
    const viewAll = screen.getByRole("button", { name: "All provider usage" })

    // Forwards from the last control wraps to the first rather than escaping
    // into the row behind the dialog.
    fireEvent.keyDown(document, { key: "Tab" })
    expect(panel.contains(document.activeElement)).toBe(true)
    expect(viewAll).toHaveFocus()

    fireEvent.keyDown(document, { key: "Tab", shiftKey: true })
    expect(panel.contains(document.activeElement)).toBe(true)
  })

  it("returns focus to the chip that opened the panel", () => {
    render(
      <ProviderUsageChips providers={[provider()]} live={liveSummary()} onViewAll={vi.fn()} />,
    )
    const chip = screen.getByRole("button", { name: /anthropic/i })

    fireEvent.click(chip)
    fireEvent.keyDown(document, { key: "Escape" })

    // Not <body>: dismissing a dialog must put the reader back where they were.
    expect(chip).toHaveFocus()
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument()
  })

  it("claims Escape so a host does not also act on it", () => {
    render(
      <ProviderUsageChips providers={[provider()]} live={liveSummary()} onViewAll={vi.fn()} />,
    )

    fireEvent.click(screen.getByRole("button", { name: /anthropic/i }))
    // `fireEvent` returns false when a handler called `preventDefault`, which
    // is the signal the popover uses to leave one Escape closing one thing.
    const notCancelled = fireEvent.keyDown(document, { key: "Escape", cancelable: true })
    expect(notCancelled).toBe(false)
  })

  it("renders a reserved state correctly if one ever arrives", () => {
    // v1 never emits `live` on the spend side, but the contract says a view
    // must not fall through to an unknown branch the day a reviewed passive
    // source does.
    render(
      <ProviderUsageChips
        providers={[provider({ state: "live" })]}
        live={liveSummary()}
        onViewAll={vi.fn()}
      />,
    )

    fireEvent.click(screen.getByRole("button", { name: /anthropic, live/i }))
    expect(screen.getByText("Live")).toBeInTheDocument()
    expect(screen.getByText(/reported this usage directly/i)).toBeInTheDocument()
  })

  it("anchors the panel above the row by default, and below it when asked", () => {
    const { container, rerender } = render(
      <ProviderUsageChips providers={[provider()]} live={liveSummary()} onViewAll={vi.fn()} />,
    )
    fireEvent.click(screen.getByRole("button", { name: /anthropic/i }))
    expect(container.querySelector('[role="dialog"]')).toHaveClass("bottom-full")

    rerender(
      <ProviderUsageChips
        providers={[provider()]}
        live={liveSummary()}
        onViewAll={vi.fn()}
        panelAnchor="down"
      />,
    )
    expect(container.querySelector('[role="dialog"]')).toHaveClass("top-full")
  })
})

/* -------------------------------------------------------------------------
 * The ring: shown only where a live window stated a percentage, and always
 * the fullest one rather than the account-wide window.
 * ---------------------------------------------------------------------- */

describe("ProviderUsageChips — the limit ring", () => {
  it("shows the fullest live window's percentage, not the account-wide one", () => {
    // The case that matters: a model-scoped weekly at 95% beside an account
    // weekly at 69%. The ring answers "how worried should I be", and the
    // per-model limit is exactly as worth a glance as the account-wide one.
    const { container } = render(
      <ProviderUsageChips
        providers={[provider({ provider: "anthropic", displayName: "Anthropic" })]}
        live={liveSummary([
          liveProvider({
            windows: [
              liveWindow({ id: "five-hour", role: "primaryShort", usedPercent: 14 }),
              liveWindow({
                id: "seven-day",
                role: "primaryLong",
                kind: "weekly",
                usedPercent: 69,
              }),
              liveWindow({
                id: "weekly-fable",
                role: "supplemental",
                kind: "weekly",
                scopeModel: "Fable",
                usedPercent: 95,
              }),
            ],
          }),
        ])}
        onViewAll={vi.fn()}
      />,
    )

    const arc = container.querySelector('[data-testid="usage-ring-arc"]')
    expect(arc).not.toBeNull()
    expect(container.querySelector('[data-testid="usage-ring-mark"]')).not.toBeNull()
    const circumference = 2 * Math.PI * 13
    expect(Number(arc?.getAttribute("stroke-dashoffset"))).toBeCloseTo(
      circumference * (1 - 95 / 100),
      5,
    )
    // The ring is a shape with no text, so the figure has to reach the
    // accessible name or a screen-reader user simply does not get it.
    expect(
      screen.getByRole("button", { name: /anthropic.*fable weekly limit 95%/i }),
    ).toBeInTheDocument()
  })

  it("keeps the glyph, with no ring, where a live window carries no percentage", () => {
    const { container } = render(
      <ProviderUsageChips
        providers={[provider({ provider: "anthropic", displayName: "Anthropic" })]}
        live={liveSummary([liveProvider({ windows: [liveWindow({ usedPercent: null })] })])}
        onViewAll={vi.fn()}
      />,
    )

    expect(container.querySelector('[data-testid="usage-ring-arc"]')).toBeNull()
    expect(screen.getByRole("button", { name: /^Anthropic,/ })).not.toHaveAccessibleName(/\d%/)
  })

  it("shows the plan limits inside the panel the chip opens", () => {
    render(
      <ProviderUsageChips
        providers={[provider({ provider: "anthropic", displayName: "Anthropic" })]}
        live={liveSummary([liveProvider({ windows: [liveWindow({ usedPercent: 88 })] })])}
        onViewAll={vi.fn()}
      />,
    )

    fireEvent.click(screen.getByRole("button", { name: /^Anthropic,/ }))
    const panel = screen.getByRole("dialog")
    expect(panel).toHaveTextContent("Plan limits")
    expect(panel).toHaveTextContent("88%")
    // And the spend half is still there, below it, since this provider does
    // have local spend to show.
    expect(panel).toHaveTextContent("Last 7 days")
  })
})

describe("ProviderUsageChips — chip value", () => {
  it("shows every live window's percentage, joined, in place of the dollar figure", () => {
    render(
      <ProviderUsageChips
        providers={[provider({ provider: "anthropic", displayName: "Anthropic" })]}
        live={liveSummary([
          liveProvider({
            windows: [
              liveWindow({ id: "five-hour", role: "primaryShort", usedPercent: 12 }),
              liveWindow({
                id: "seven-day",
                role: "primaryLong",
                kind: "weekly",
                usedPercent: 19,
              }),
            ],
          }),
        ])}
        onViewAll={vi.fn()}
      />,
    )

    // five-hour 12%, weekly (seven-day) 19% — in `liveWindows` order, not the
    // dollar spend the provider prop carries.
    expect(screen.getByRole("button", { name: /^Anthropic,/ })).toHaveTextContent("12% / 19%")
  })

  it("names each window's percentage in the accessible name, and drops the dollar clause", () => {
    render(
      <ProviderUsageChips
        providers={[provider({ provider: "anthropic", displayName: "Anthropic" })]}
        live={liveSummary([
          liveProvider({
            windows: [
              liveWindow({ id: "five-hour", role: "primaryShort", usedPercent: 12 }),
              liveWindow({
                id: "seven-day",
                role: "primaryLong",
                kind: "weekly",
                usedPercent: 19,
              }),
            ],
          }),
        ])}
        onViewAll={vi.fn()}
      />,
    )

    expect(
      screen.getByRole("button", {
        name: "Anthropic, estimated, 5-hour limit 12%, weekly limit 19%",
      }),
    ).toBeInTheDocument()
  })
})

describe("ProviderUsageChips — hover", () => {
  const chip = () => screen.getByRole("button", { name: /^Anthropic,/ })

  function chips() {
    render(
      <ProviderUsageChips
        providers={[provider({ provider: "anthropic", displayName: "Anthropic" })]}
        live={liveSummary([liveProvider({ windows: [liveWindow({ usedPercent: 88 })] })])}
        onViewAll={vi.fn()}
      />,
    )
  }

  beforeEach(() => vi.useFakeTimers({ shouldAdvanceTime: true }))
  afterEach(() => vi.useRealTimers())

  it("waits before opening, so a pointer crossing the row lights nothing up", () => {
    chips()
    fireEvent.pointerEnter(chip(), { pointerType: "mouse" })
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument()

    act(() => void vi.advanceTimersByTime(200))
    expect(screen.getByRole("dialog")).toBeInTheDocument()
  })

  it("does not claim to be modal, and does not steal focus, when hover opened it", () => {
    // Yanking focus out from under a pointer that merely passed over a chip is
    // hostile, and a modal nobody asked for is worse than a disclosure.
    chips()
    fireEvent.pointerEnter(chip(), { pointerType: "mouse" })
    act(() => void vi.advanceTimersByTime(200))

    const panel = screen.getByRole("dialog")
    expect(panel).not.toHaveAttribute("aria-modal")
    expect(panel.contains(document.activeElement)).toBe(false)
  })

  it("survives the diagonal from the chip into the panel", () => {
    chips()
    fireEvent.pointerEnter(chip(), { pointerType: "mouse" })
    act(() => void vi.advanceTimersByTime(200))

    fireEvent.pointerLeave(chip(), { pointerType: "mouse" })
    // Reaching the panel before the close lands keeps it open.
    act(() => void vi.advanceTimersByTime(100))
    fireEvent.pointerEnter(screen.getByRole("dialog"), { pointerType: "mouse" })
    act(() => void vi.advanceTimersByTime(500))
    expect(screen.getByRole("dialog")).toBeInTheDocument()
  })

  it("closes once the pointer leaves the panel too", () => {
    chips()
    fireEvent.pointerEnter(chip(), { pointerType: "mouse" })
    act(() => void vi.advanceTimersByTime(200))

    fireEvent.pointerLeave(screen.getByRole("dialog"), { pointerType: "mouse" })
    act(() => void vi.advanceTimersByTime(140))
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument()
  })

  it("keeps a deliberately opened panel open when the pointer wanders off", () => {
    // A click is a decision; a pointer moving away is not a retraction of it.
    chips()
    fireEvent.click(chip())
    expect(screen.getByRole("dialog")).toHaveAttribute("aria-modal", "true")

    fireEvent.pointerLeave(chip(), { pointerType: "mouse" })
    act(() => void vi.advanceTimersByTime(1_000))
    expect(screen.getByRole("dialog")).toBeInTheDocument()
  })

  it("ignores hover from a touch pointer, which fires one just before its tap", () => {
    chips()
    fireEvent.pointerEnter(chip(), { pointerType: "touch" })
    act(() => void vi.advanceTimersByTime(500))
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument()

    // The tap itself still opens it.
    fireEvent.click(chip())
    expect(screen.getByRole("dialog")).toBeInTheDocument()
  })
})
