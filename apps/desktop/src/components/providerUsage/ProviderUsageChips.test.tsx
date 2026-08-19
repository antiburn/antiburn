// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { act, fireEvent, render, screen } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import type {
  LiveUsageSummaryPayload,
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

/** Providers ranked below the first, so the overflow affordance appears. */
function ranked(count: number): ProviderUsagePayload[] {
  return Array.from({ length: count }, (_, index) => {
    const window = usageWindow({ estimatedUsd: count - index, sessionCount: 1 })
    return provider({
      provider: `p${index}`,
      displayName: `Provider ${index}`,
      windows: { today: window, week: window, month: window },
    })
  })
}

describe("ProviderUsageChips", () => {
  it("shows a chip per provider used today, with the figure in its name", () => {
    render(<ProviderUsageChips providers={[provider()]} onViewAll={vi.fn()} />)

    // The chip renders a glyph and a number; everything a reader needs to know
    // about what that number *is* has to be in the accessible name.
    expect(
      screen.getByRole("button", { name: "Anthropic, $1.25 today, estimated" }),
    ).toBeInTheDocument()
  })

  it("shows a token count when the provider could not be priced", () => {
    const window = usageWindow({ tokensIn: 12_000, sessionCount: 2 })
    render(
      <ProviderUsageChips
        providers={[
          provider({
            state: "observed",
            windows: { today: window, week: window, month: window },
          }),
        ]}
        onViewAll={vi.fn()}
      />,
    )

    expect(
      screen.getByRole("button", { name: "Anthropic, 12.0k today, observed" }),
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
        onViewAll={vi.fn()}
      />,
    )

    expect(
      screen.getByRole("button", {
        name: /anthropic, \$1\.25 today, estimated, last used 2d ago/i,
      }),
    ).toBeInTheDocument()
  })

  it("says so honestly when nothing was used today", () => {
    const idle = provider({
      windows: {
        today: usageWindow(),
        week: usageWindow({ tokensIn: 5 }),
        month: usageWindow(),
      },
    })
    render(<ProviderUsageChips providers={[idle]} onViewAll={vi.fn()} />)

    expect(screen.getByText("No provider usage today")).toBeInTheDocument()
    expect(screen.queryByRole("button", { name: /anthropic/i })).not.toBeInTheDocument()
  })

  it("collapses everything past the chip budget into one overflow affordance", () => {
    const onViewAll = vi.fn()
    render(<ProviderUsageChips providers={ranked(5)} onViewAll={onViewAll} maxVisible={3} />)

    expect(screen.getAllByRole("button", { name: /^Provider \d/ })).toHaveLength(3)
    const overflow = screen.getByRole("button", { name: "Show 2 more providers" })
    fireEvent.click(overflow)
    expect(onViewAll).toHaveBeenCalledTimes(1)
  })

  it("opens a provider panel on click and closes it on a second click", () => {
    render(<ProviderUsageChips providers={[provider()]} onViewAll={vi.fn()} />)
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

  it("closes the panel on Escape and on a press outside it", () => {
    render(<ProviderUsageChips providers={[provider()]} onViewAll={vi.fn()} />)
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
    render(<ProviderUsageChips providers={[provider()]} onViewAll={onViewAll} />)

    fireEvent.click(screen.getByRole("button", { name: /anthropic/i }))
    fireEvent.click(screen.getByRole("button", { name: "All provider usage" }))
    expect(onViewAll).toHaveBeenCalledTimes(1)
    // Navigating away also dismisses the panel, so returning shows the list.
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument()
  })

  it("moves focus into the panel it opens", () => {
    render(<ProviderUsageChips providers={[provider()]} onViewAll={vi.fn()} />)

    fireEvent.click(screen.getByRole("button", { name: /anthropic/i }))

    // `role="dialog"` promises containment. A reader who is not in the dialog
    // has been told about a surface they cannot reach.
    const panel = screen.getByRole("dialog")
    expect(panel.contains(document.activeElement)).toBe(true)
    expect(screen.getByRole("button", { name: "All provider usage" })).toHaveFocus()
  })

  it("holds Tab inside the panel while it is open", () => {
    render(<ProviderUsageChips providers={[provider()]} onViewAll={vi.fn()} />)

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
    render(<ProviderUsageChips providers={[provider()]} onViewAll={vi.fn()} />)
    const chip = screen.getByRole("button", { name: /anthropic/i })

    fireEvent.click(chip)
    fireEvent.keyDown(document, { key: "Escape" })

    // Not <body>: dismissing a dialog must put the reader back where they were.
    expect(chip).toHaveFocus()
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument()
  })

  it("claims Escape so a host does not also act on it", () => {
    render(<ProviderUsageChips providers={[provider()]} onViewAll={vi.fn()} />)

    fireEvent.click(screen.getByRole("button", { name: /anthropic/i }))
    // `fireEvent` returns false when a handler called `preventDefault`, which
    // is the signal the popover uses to leave one Escape closing one thing.
    const notCancelled = fireEvent.keyDown(document, { key: "Escape", cancelable: true })
    expect(notCancelled).toBe(false)
  })

  it("renders a reserved state correctly if one ever arrives", () => {
    // v1 never emits `live`, but the contract says a view must not fall through
    // to an unknown branch the day a reviewed passive source does.
    render(<ProviderUsageChips providers={[provider({ state: "live" })]} onViewAll={vi.fn()} />)

    fireEvent.click(screen.getByRole("button", { name: /anthropic, \$1\.25 today, live/i }))
    expect(screen.getByText("Live")).toBeInTheDocument()
    expect(screen.getByText(/reported this usage directly/i)).toBeInTheDocument()
  })

  it("anchors the panel above the row by default, and below it when asked", () => {
    const { container, rerender } = render(
      <ProviderUsageChips providers={[provider()]} onViewAll={vi.fn()} />,
    )
    fireEvent.click(screen.getByRole("button", { name: /anthropic/i }))
    expect(container.querySelector('[role="dialog"]')).toHaveClass("bottom-full")

    rerender(
      <ProviderUsageChips providers={[provider()]} onViewAll={vi.fn()} panelAnchor="down" />,
    )
    expect(container.querySelector('[role="dialog"]')).toHaveClass("top-full")
  })
})

/* -------------------------------------------------------------------------
 * The ring: shown only where a provider stated a percentage
 * ---------------------------------------------------------------------- */

function liveSummary(usedPercent: number | null = 88): LiveUsageSummaryPayload {
  const forecast = {
    unavailableReason: "sparseHistory",
    confidence: null,
    consumptionRate: null,
    paceRatio: null,
    paceTrend: null,
    runwayAt: null,
    usedToday: null,
  }
  return {
    providers: [
      {
        provider: "anthropic",
        displayName: "Anthropic",
        support: "live",
        freshness: "fresh",
        sourceLabel: "Asked Claude directly",
        observedAt: new Date().toISOString(),
        windows: [
          {
            id: "five-hour",
            role: "primaryShort",
            kind: "rolling",
            scopeModel: null,
            usedPercent: 12,
            startsAt: null,
            resetsAt: null,
            hasNonzeroUsageInCurrentPeriod: false,
            forecast,
          },
          {
            id: "seven-day",
            role: "primaryLong",
            kind: "weekly",
            scopeModel: null,
            usedPercent,
            startsAt: null,
            resetsAt: null,
            hasNonzeroUsageInCurrentPeriod: false,
            forecast,
          },
        ],
        extraUsage: null,
      },
    ],
    errors: [],
    generatedAt: new Date().toISOString(),
  }
}

describe("ProviderUsageChips — the limit ring", () => {
  it("replaces the glyph with a ring at the account-wide window", () => {
    // Not the shortest and not the fullest: a single ring has to answer "how
    // am I doing", which only the account-wide window answers.
    const { container } = render(
      <ProviderUsageChips
        providers={[provider({ provider: "anthropic", displayName: "Anthropic" })]}
        live={liveSummary(88)}
        onViewAll={vi.fn()}
      />,
    )

    expect(container.querySelector('[data-testid="usage-ring-arc"]')).not.toBeNull()
    // The ring carries the provider's identity rather than replacing it — the
    // brand mark where one exists, so the chip still says whose limit it is.
    expect(container.querySelector('[data-testid="usage-ring-mark"]')).not.toBeNull()
    // The ring is a shape with no text, so the figure has to reach the
    // accessible name or a screen-reader user simply does not get it.
    expect(
      screen.getByRole("button", { name: /anthropic.*weekly limit 88%/i }),
    ).toBeInTheDocument()
  })

  it("keeps the glyph, and says nothing extra, where no limit was stated", () => {
    const { container } = render(
      <ProviderUsageChips
        providers={[provider({ provider: "openai", displayName: "OpenAI" })]}
        live={liveSummary(88)}
        onViewAll={vi.fn()}
      />,
    )

    expect(container.querySelector('[data-testid="usage-ring-arc"]')).toBeNull()
    expect(screen.getByRole("button", { name: /^OpenAI,/ })).not.toHaveAccessibleName(/limit/i)
  })

  it("shows the plan limits inside the panel the chip opens", () => {
    render(
      <ProviderUsageChips
        providers={[provider({ provider: "anthropic", displayName: "Anthropic" })]}
        live={liveSummary(88)}
        onViewAll={vi.fn()}
      />,
    )

    fireEvent.click(screen.getByRole("button", { name: /^Anthropic,/ }))
    const panel = screen.getByRole("dialog")
    expect(panel).toHaveTextContent("Plan limits")
    expect(panel).toHaveTextContent("88%")
    // And the spend half is still there, below it.
    expect(panel).toHaveTextContent("Last 7 days")
  })
})

describe("ProviderUsageChips — hover", () => {
  const chip = () => screen.getByRole("button", { name: /^Anthropic,/ })

  function chips() {
    render(
      <ProviderUsageChips
        providers={[provider({ provider: "anthropic", displayName: "Anthropic" })]}
        live={liveSummary(88)}
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
