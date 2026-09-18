import { cleanup, fireEvent, render, screen } from "@testing-library/react"
import { afterEach, describe, expect, it, vi } from "vitest"

import type { SessionQuotaEntryPayload, SessionQuotaPayload } from "../../lib/ipc"
import { SessionQuotaSection } from "./SessionQuotaSection"

afterEach(cleanup)

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
      peakPercent: 40,
    },
    usd: 2.5,
    percent: 12,
    confidence: "learned",
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
    ...over,
  }
}

function payload(entries: SessionQuotaEntryPayload[]): SessionQuotaPayload {
  return { entries, generatedAt: "g" }
}

describe("SessionQuotaSection", () => {
  it("renders one row per window for a two-window session", () => {
    render(
      <SessionQuotaSection
        sessionQuota={payload([
          boundEntry({ lane: "fiveHour", laneLabel: "5-hour", period: boundEntry().period }),
          boundEntry({
            lane: "fiveHour",
            laneLabel: "5-hour",
            period: {
              periodId: 2,
              startsAtEpoch: 1_700_000,
              resetsAtEpoch: 1_718_000,
              startSource: "reported",
              resetSource: "reported",
              peakPercent: 20,
            },
          }),
        ])}
        sessionQuotaError={false}
        onOpenQuota={() => undefined}
      />,
    )
    expect(screen.getAllByRole("button")).toHaveLength(2)
  })

  it("shows the unattributed copy for an unbound entry", () => {
    render(
      <SessionQuotaSection
        sessionQuota={payload([unboundEntry()])}
        sessionQuotaError={false}
        onOpenQuota={() => undefined}
      />,
    )
    expect(screen.getByText("Not linked to an account")).toBeTruthy()
    expect(screen.getByText("Unattributed")).toBeTruthy()
    expect(screen.queryByRole("button")).toBeNull()
  })

  it("calls onOpenQuota with the clicked window's range on click", () => {
    const onOpenQuota = vi.fn()
    render(
      <SessionQuotaSection
        sessionQuota={payload([boundEntry()])}
        sessionQuotaError={false}
        onOpenQuota={onOpenQuota}
      />,
    )
    fireEvent.click(screen.getByRole("button"))
    expect(onOpenQuota).toHaveBeenCalledWith({
      provider: "anthropic",
      accountKey: "acct-1",
      lane: "weekly",
      rangeStart: 1_000_000,
      rangeEnd: 1_604_800,
    })
  })

  it("shows the empty copy when there are no entries and no error", () => {
    render(
      <SessionQuotaSection
        sessionQuota={payload([])}
        sessionQuotaError={false}
        onOpenQuota={() => undefined}
      />,
    )
    expect(screen.getByText("No quota windows recorded for this session.")).toBeTruthy()
  })

  it("renders nothing when quota has not loaded yet", () => {
    const { container } = render(
      <SessionQuotaSection
        sessionQuota={null}
        sessionQuotaError={false}
        onOpenQuota={() => undefined}
      />,
    )
    expect(container).toBeEmptyDOMElement()
  })

  it("renders nothing on an empty result carrying an error", () => {
    const { container } = render(
      <SessionQuotaSection
        sessionQuota={payload([])}
        sessionQuotaError
        onOpenQuota={() => undefined}
      />,
    )
    expect(container).toBeEmptyDOMElement()
  })
})
