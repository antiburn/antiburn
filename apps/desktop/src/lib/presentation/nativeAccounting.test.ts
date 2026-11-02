import { describe, expect, it } from "vitest"

import { nativeAccountingRows } from "./nativeAccounting"
import type { NativeSourceAccounting, NativeTokenClasses } from "../types/session"

const tokens: NativeTokenClasses = {
  inputTokens: 10,
  visibleOutputTokens: 3,
  reasoningTokens: 2,
  cacheReadTokens: 7,
  cacheWriteTokens: 5,
}

function accounting(): NativeSourceAccounting {
  return {
    aggregateTokens: { ...tokens, inputTokens: 15, reasoningTokens: 6 },
    retainedTokens: tokens,
    aggregateCalculatedCost: { usd: 0.04, provenance: "producerCalculated" },
    retainedCalculatedCost: { usd: 0.01, provenance: "producerCalculated" },
  }
}

describe("native accounting detail", () => {
  it("keeps separate output classes and shows unallocated recorded amounts", () => {
    const rows = Object.fromEntries(
      nativeAccountingRows(
        accounting(),
        {
          status: "unallocated",
          value: {
            ...tokens,
            inputTokens: 5,
            visibleOutputTokens: 0,
            reasoningTokens: 4,
            cacheReadTokens: 0,
            cacheWriteTokens: 0,
          },
        },
        { status: "unallocated", value: { usd: 0.03, provenance: "producerCalculated" } },
      ),
    )
    expect(rows["Retained token classes"]).toContain("Visible output 3 · Reasoning 2")
    expect(rows["Unallocated token classes"]).toContain(
      "Input 5 · Visible output 0 · Reasoning 4",
    )
    expect(rows["Unallocated calculated cost"]).toBe("$0.030000")
  })

  it("keeps unknown, inconsistent, and zero calculated values distinct", () => {
    const value = accounting()
    value.aggregateTokens = null
    value.aggregateCalculatedCost = null
    value.retainedCalculatedCost = { usd: 0, provenance: "producerCalculated" }
    let rows = Object.fromEntries(nativeAccountingRows(value))
    expect(rows["Unallocated token classes"]).toBe("Unknown")
    expect(rows["Retained calculated cost"]).toBe("$0.000000")
    value.aggregateTokens = { ...tokens, reasoningTokens: 1 }
    value.aggregateCalculatedCost = { usd: 0, provenance: "producerCalculated" }
    value.retainedCalculatedCost.usd = 1
    rows = Object.fromEntries(
      nativeAccountingRows(value, { status: "inconsistent" }, { status: "inconsistent" }),
    )
    expect(rows["Unallocated token classes"]).toBe("Inconsistent recorded totals")
    expect(rows["Unallocated calculated cost"]).toBe("Inconsistent recorded totals")
  })

  it("does not invent precise classes above JavaScript's safe integer range", () => {
    const value = accounting()
    value.aggregateTokens = { ...tokens, inputTokens: Number.MAX_SAFE_INTEGER + 1 }
    expect(
      Object.fromEntries(
        nativeAccountingRows(value, { status: "unallocated", value: value.aggregateTokens }),
      )["Unallocated token classes"],
    ).toBe("Unavailable at this precision")
  })
})
