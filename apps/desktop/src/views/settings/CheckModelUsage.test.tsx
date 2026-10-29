import { render, screen } from "@testing-library/react"
import { expect, it } from "vitest"
import type { CheckUsageSummary } from "../../lib/checkAvailability"
import { CheckModelUsage } from "./CheckModelUsage"

const emptyUsage: CheckUsageSummary = {
  confirmedCalls: 0,
  inputTokens: 0,
  outputTokens: 0,
  cacheHits: 0,
  unknownOutcomes: 0,
  estimatedUsd: "$0.00",
  lastUsedAtEpoch: null,
}
const recordedUsage: CheckUsageSummary = {
  ...emptyUsage,
  confirmedCalls: 2,
  inputTokens: 1234,
  outputTokens: 56,
  cacheHits: 7,
  estimatedUsd: "$0.00000017",
}

it.each([0, 3])("keeps exact recorded usage with %s unknown attempts", (unknownOutcomes) => {
  const { container } = render(
    <CheckModelUsage
      usage={{ ...recordedUsage, unknownOutcomes }}
      loading={false}
      stale={false}
      control="checkModelUsage"
    />,
  )
  expect(screen.getByText(/2 requests with recorded usage/)).toHaveTextContent(
    "2 requests with recorded usage · $0.00000017 estimated",
  )
  expect(screen.getByText("Input tokens").nextElementSibling).toHaveTextContent("1,234")
  expect(screen.getByText("Output tokens").nextElementSibling).toHaveTextContent("56")
  expect(screen.getByText("Cached results reused").nextElementSibling).toHaveTextContent("7")
  expect(
    screen.getByText("Recorded device usage across connections. Not a bill."),
  ).toBeVisible()
  expect(container).not.toHaveTextContent(/unknown|unavailable|device totals|not included/i)
})

it("omits an absent estimate without dropping known usage or adding a cost status", () => {
  const { container } = render(
    <CheckModelUsage
      usage={{ ...recordedUsage, unknownOutcomes: 3, estimatedUsd: null }}
      loading={false}
      stale={false}
      control="checkModelUsage"
    />,
  )
  expect(screen.getByText("2 requests with recorded usage")).toBeVisible()
  expect(screen.getByText("Input tokens").nextElementSibling).toHaveTextContent("1,234")
  expect(container).not.toHaveTextContent(/unknown|unavailable|estimate|\$/i)
})

it.each([null, "$0.00"])("omits unknown-only usage with estimate %s", (estimatedUsd) => {
  const { container } = render(
    <CheckModelUsage
      usage={{ ...emptyUsage, unknownOutcomes: 2, estimatedUsd }}
      loading={false}
      stale={false}
      control="checkModelUsage"
    />,
  )
  expect(container).toBeEmptyDOMElement()
})

it.each([
  { usage: null, loading: false, stale: false },
  { usage: null, loading: true, stale: false },
  { usage: recordedUsage, loading: false, stale: true },
  { usage: recordedUsage, loading: true, stale: false },
])("omits untrusted or absent current usage: %o", (props) => {
  const { container } = render(<CheckModelUsage {...props} control="checkModelUsage" />)
  expect(container).toBeEmptyDOMElement()
})

it("uses the normal empty sentence only for current zero usage without unknown attempts", () => {
  render(
    <CheckModelUsage
      usage={emptyUsage}
      loading={false}
      stale={false}
      control="checkModelUsage"
    />,
  )
  expect(screen.getByText("No model requests recorded on this device.")).toBeVisible()
  expect(screen.queryByText("Input tokens")).not.toBeInTheDocument()
  expect(screen.queryByText(/\$0|0 requests|unknown/i)).not.toBeInTheDocument()
})

it("keeps known zero tokens for a recorded call and known cache reuse without a call", () => {
  const { rerender } = render(
    <CheckModelUsage
      usage={{ ...emptyUsage, confirmedCalls: 1, estimatedUsd: null }}
      loading={false}
      stale={false}
      control="checkModelUsage"
    />,
  )
  expect(screen.getByText("1 request with recorded usage")).toBeVisible()
  expect(screen.getByText("Input tokens").nextElementSibling).toHaveTextContent("0")
  rerender(
    <CheckModelUsage
      usage={{ ...emptyUsage, cacheHits: 4, estimatedUsd: null }}
      loading={false}
      stale={false}
      control="checkModelUsage"
    />,
  )
  expect(screen.getByText("Cached results reused").nextElementSibling).toHaveTextContent("4")
  expect(
    screen.queryByText("No model requests recorded on this device."),
  ).not.toBeInTheDocument()
})
