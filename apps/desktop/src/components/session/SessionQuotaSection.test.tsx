import { cleanup, fireEvent, render, screen } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import type { SessionQuotaEntryPayload, SessionQuotaPayload } from "../../lib/ipc"
import { limitsExpandedStore } from "./limitsExpandedStore"
import { orderLimitEntries, SessionQuotaSection } from "./SessionQuotaSection"

afterEach(cleanup)

// The expanded flag is app-wide, so reset it between tests to keep them independent.
beforeEach(() => {
  limitsExpandedStore.set(false)
})

const HEADER_NAME = /^Limits/

function boundEntry(over: Partial<SessionQuotaEntryPayload> = {}): SessionQuotaEntryPayload {
  return {
    provider: "anthropic",
    displayName: "Claude",
    accountKey: "acct-1",
    lane: "weekly",
    laneLabel: "Weekly",
    period: {
      periodId: 1,
      startsAtEpoch: 1_000_000,
      resetsAtEpoch: 1_604_800,
      startSource: "reported",
      resetSource: "reported",
    },
    usd: 2.5,
    percent: 12,
    confidence: "learned",
    plan: null,
    ...over,
  }
}

function unboundEntry(over: Partial<SessionQuotaEntryPayload> = {}): SessionQuotaEntryPayload {
  return {
    provider: "openai",
    displayName: "Codex",
    accountKey: null,
    lane: null,
    laneLabel: null,
    period: null,
    usd: 0.4,
    percent: null,
    confidence: "unbound",
    plan: null,
    ...over,
  }
}

function payload(entries: SessionQuotaEntryPayload[]): SessionQuotaPayload {
  return { entries, generatedAt: "g" }
}

/** Several distinct windows on the same account, for the header-summary tests. */
function sevenWindows(): SessionQuotaEntryPayload[] {
  const percents = [10, 20, 30, 40, 50, 60, 88]
  return percents.map((percent, index) =>
    boundEntry({
      lane: `lane-${index}`,
      laneLabel: `Lane ${index}`,
      period: {
        periodId: index,
        startsAtEpoch: 1_000_000 + index,
        resetsAtEpoch: 1_604_800 + index,
        startSource: "reported",
        resetSource: "reported",
      },
      percent,
    }),
  )
}

describe("SessionQuotaSection", () => {
  it("renders nothing when quota has not loaded yet", () => {
    const { container } = render(
      <SessionQuotaSection sessionQuota={null} onOpenQuota={() => undefined} />,
    )
    expect(container).toBeEmptyDOMElement()
  })

  it("renders nothing on an empty result", () => {
    const { container } = render(
      <SessionQuotaSection sessionQuota={payload([])} onOpenQuota={() => undefined} />,
    )
    expect(container).toBeEmptyDOMElement()
  })

  it("is collapsed by default: the header is present but no row is in the DOM", () => {
    render(
      <SessionQuotaSection
        sessionQuota={payload([boundEntry()])}
        onOpenQuota={() => undefined}
      />,
    )
    const header = screen.getByRole("button", { name: HEADER_NAME })
    expect(header).toHaveAttribute("aria-expanded", "false")
    // The header is the only button while collapsed: the row buttons are not rendered.
    expect(screen.getAllByRole("button")).toHaveLength(1)
  })

  it("clicking the header expands the section and shows its rows", () => {
    render(
      <SessionQuotaSection
        sessionQuota={payload([boundEntry()])}
        onOpenQuota={() => undefined}
      />,
    )
    const header = screen.getByRole("button", { name: HEADER_NAME })
    fireEvent.click(header)
    expect(header).toHaveAttribute("aria-expanded", "true")
    expect(screen.getAllByRole("button")).toHaveLength(2)
    expect(screen.getByText("Weekly")).toBeTruthy()
  })

  it("summarizes the window count in the header", () => {
    render(
      <SessionQuotaSection
        sessionQuota={payload(sevenWindows())}
        onOpenQuota={() => undefined}
      />,
    )
    expect(screen.getByText("contributed to 7 separate windows")).toBeTruthy()
  })

  it("uses singular 'window' for a single entry", () => {
    render(
      <SessionQuotaSection
        sessionQuota={payload([boundEntry({ percent: 40 })])}
        onOpenQuota={() => undefined}
      />,
    )
    expect(screen.getByText("contributed to 1 window")).toBeTruthy()
  })

  it("counts only bound entries as windows, not an unbound one alongside them", () => {
    render(
      <SessionQuotaSection
        sessionQuota={payload([
          boundEntry({ lane: "lane-a" }),
          boundEntry({ lane: "lane-b" }),
          unboundEntry(),
        ])}
        onOpenQuota={() => undefined}
      />,
    )
    expect(screen.getByText("contributed to 2 separate windows")).toBeTruthy()
  })

  it("renders no window count when every entry is unbound", () => {
    render(
      <SessionQuotaSection
        sessionQuota={payload([unboundEntry(), unboundEntry({ provider: "google" })])}
        onOpenQuota={() => undefined}
      />,
    )
    expect(screen.queryByText((text) => text.startsWith("contributed to"))).toBeNull()
  })

  it("shows the Measured tag for a shared-meter entry", () => {
    render(
      <SessionQuotaSection
        sessionQuota={payload([boundEntry({ confidence: "measured" })])}
        onOpenQuota={() => undefined}
      />,
    )
    fireEvent.click(screen.getByRole("button", { name: HEADER_NAME }))
    expect(screen.getByText("Measured")).toBeTruthy()
  })

  it("shows the unattributed copy for an unbound entry", () => {
    render(
      <SessionQuotaSection
        sessionQuota={payload([unboundEntry()])}
        onOpenQuota={() => undefined}
      />,
    )
    fireEvent.click(screen.getByRole("button", { name: HEADER_NAME }))
    expect(screen.getByText("Not linked to an account")).toBeTruthy()
    expect(screen.getByText("Unattributed")).toBeTruthy()
    // The unbound row has no button of its own, only the (now-expanded) header.
    expect(screen.getAllByRole("button")).toHaveLength(1)
  })

  it("calls onOpenQuota with the clicked window's range on click", () => {
    const onOpenQuota = vi.fn()
    render(
      <SessionQuotaSection sessionQuota={payload([boundEntry()])} onOpenQuota={onOpenQuota} />,
    )
    fireEvent.click(screen.getByRole("button", { name: HEADER_NAME }))
    // Clicking inner row text bubbles to the row's own button handler.
    fireEvent.click(screen.getByText("Weekly"))
    expect(onOpenQuota).toHaveBeenCalledWith({
      provider: "anthropic",
      accountKey: "acct-1",
      lane: "weekly",
      rangeStart: 1_000_000,
      rangeEnd: 1_604_800,
    })
  })

  it("gives the meter's wrapper the wider w-36 track, clear of the percent column", () => {
    const { container } = render(
      <SessionQuotaSection
        sessionQuota={payload([boundEntry()])}
        onOpenQuota={() => undefined}
      />,
    )
    fireEvent.click(screen.getByRole("button", { name: HEADER_NAME }))
    expect(container.querySelector(".w-36")).not.toBeNull()
    expect(container.querySelector(".w-24")).toBeNull()
  })

  it("shows the account's plan label in the heading, alongside its name", () => {
    render(
      <SessionQuotaSection
        sessionQuota={payload([
          boundEntry({ plan: { name: "max", tier: "default_claude_max_20x" } }),
        ])}
        onOpenQuota={() => undefined}
      />,
    )
    fireEvent.click(screen.getByRole("button", { name: HEADER_NAME }))
    const heading = screen.getByText(/Claude/, { selector: "h4" })
    expect(heading.textContent).toBe("Claude · Max 20x")
  })

  it("shows only the account name when no observation has named a plan", () => {
    render(
      <SessionQuotaSection
        sessionQuota={payload([boundEntry()])}
        onOpenQuota={() => undefined}
      />,
    )
    fireEvent.click(screen.getByRole("button", { name: HEADER_NAME }))
    const heading = screen.getByText("Claude", { selector: "h4" })
    expect(heading.textContent).toBe("Claude")
  })

  describe("orderLimitEntries", () => {
    it("orders bound entries newest window first", () => {
      const older = boundEntry({
        lane: "older",
        period: { ...boundEntry().period!, startsAtEpoch: 1_000 },
      })
      const newer = boundEntry({
        lane: "newer",
        period: { ...boundEntry().period!, startsAtEpoch: 2_000 },
      })
      expect(orderLimitEntries([older, newer])).toEqual([newer, older])
    })

    it("breaks a tied start by window length, shorter first", () => {
      const long = boundEntry({
        lane: "long",
        period: { ...boundEntry().period!, startsAtEpoch: 0, resetsAtEpoch: 604_800 },
      })
      const short = boundEntry({
        lane: "short",
        period: { ...boundEntry().period!, startsAtEpoch: 0, resetsAtEpoch: 18_000 },
      })
      expect(orderLimitEntries([long, short])).toEqual([short, long])
    })

    it("places every unbound entry after every bound one", () => {
      const bound = boundEntry({ lane: "bound" })
      const unbound = unboundEntry()
      expect(orderLimitEntries([unbound, bound])).toEqual([bound, unbound])
    })
  })

  describe("the current window", () => {
    const GENERATED_AT = new Date(1_000_000 * 1000 + 3_600_000).toISOString()

    it("marks an open window as current: caption, aria-current, and the selected background", () => {
      const { container } = render(
        <SessionQuotaSection
          sessionQuota={{ entries: [boundEntry()], generatedAt: GENERATED_AT }}
          onOpenQuota={() => undefined}
        />,
      )
      fireEvent.click(screen.getByRole("button", { name: HEADER_NAME }))
      const row = screen.getByRole("button", { name: /Weekly/ })
      expect(row).toHaveAttribute("aria-current", "true")
      expect(screen.getByText("current")).toBeTruthy()
      expect(container.querySelector(".bg-surface-selected\\/40")).not.toBeNull()
    })

    it("leaves a closed window with no current caption, aria-current, or selected background", () => {
      const { container } = render(
        <SessionQuotaSection
          sessionQuota={{
            entries: [
              boundEntry({ period: { ...boundEntry().period!, resetsAtEpoch: 500_000 } }),
            ],
            generatedAt: GENERATED_AT,
          }}
          onOpenQuota={() => undefined}
        />,
      )
      fireEvent.click(screen.getByRole("button", { name: HEADER_NAME }))
      const row = screen.getByRole("button", { name: /Weekly/ })
      expect(row).not.toHaveAttribute("aria-current")
      expect(screen.queryByText("current")).toBeNull()
      expect(container.querySelector(".bg-surface-selected\\/40")).toBeNull()
    })
  })
})
