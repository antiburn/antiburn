import { describe, expect, it } from "vitest"

import {
  modelMatchesScope,
  modelRunName,
  modelRunNames,
  modelRunShortNames,
  modelRunShortPairs,
  modelShortName,
} from "./models"

describe("modelShortName", () => {
  it("removes the Claude and GPT family prefixes", () => {
    expect(modelShortName("claude-fable-5")).toBe("fable-5")
    expect(modelShortName("claude-opus-4-6")).toBe("opus-4-6")
    expect(modelShortName("gpt-5.6-sol")).toBe("5.6-sol")
  })

  it("removes known provider wrappers", () => {
    expect(modelShortName("anthropic/claude-opus-4.8")).toBe("opus-4.8")
    expect(modelShortName("openai.gpt-5.6-terra")).toBe("5.6-terra")
    expect(modelShortName("antigravity-claude-opus-4-6-thinking")).toBe("opus-4-6-thinking")
  })

  it("keeps an unknown model ID unchanged", () => {
    expect(modelShortName("composer-2.5-fast")).toBe("composer-2.5-fast")
  })
})

describe("modelRunNames", () => {
  it("formats the thinking mode and removes duplicate labels", () => {
    expect(
      modelRunNames([
        { model: "gpt-5.6-sol", thinkingMode: "xhigh" },
        { model: "gpt-5.6-sol", thinkingMode: "xhigh" },
        { model: "claude-fable-5" },
      ]),
    ).toEqual(["gpt-5.6-sol/xhigh", "claude-fable-5"])
    expect(modelRunName({ model: "gpt-5.6-sol", thinkingMode: " " })).toBe("gpt-5.6-sol")
  })

  it("shortens model IDs without changing run order", () => {
    expect(
      modelRunShortNames([
        { model: "gpt-5.6-sol", thinkingMode: "xhigh" },
        { model: "claude-fable-5", thinkingMode: "high" },
      ]),
    ).toEqual(["5.6-sol/xhigh", "fable-5/high"])
  })

  it("keeps names and modes apart in short pairs, with the same duplicate removal", () => {
    expect(
      modelRunShortPairs([
        { model: "gpt-5.6-sol", thinkingMode: "xhigh" },
        { model: "gpt-5.6-sol", thinkingMode: "xhigh" },
        { model: "claude-fable-5" },
        { model: "claude-fable-5", thinkingMode: " " },
      ]),
    ).toEqual([{ model: "5.6-sol", thinkingMode: "xhigh" }, { model: "fable-5" }])
  })
})

describe("modelMatchesScope", () => {
  it("matches a scope name to the model ids of its family", () => {
    expect(modelMatchesScope("claude-fable-5", "Fable")).toBe(true)
    expect(modelMatchesScope("claude-fable-5-1", "Fable")).toBe(true)
  })

  it("leaves another model of the same vendor out", () => {
    // The report this rule answers: a Claude Code session on Opus must not
    // sweep the Fable meter.
    expect(modelMatchesScope("claude-opus-4-6", "Fable")).toBe(false)
    expect(modelMatchesScope("gpt-5.6-sol", "Fable")).toBe(false)
  })

  it("keeps the version apart from the family", () => {
    expect(modelMatchesScope("claude-sonnet-4-5-20250929", "Claude Sonnet 4.5")).toBe(true)
    expect(modelMatchesScope("claude-sonnet-3-7", "Claude Sonnet 4.5")).toBe(false)
  })

  it("matches nothing for a scope name that states only a vendor", () => {
    expect(modelMatchesScope("claude-fable-5", "Claude")).toBe(false)
    expect(modelMatchesScope("claude-fable-5", "")).toBe(false)
  })
})
