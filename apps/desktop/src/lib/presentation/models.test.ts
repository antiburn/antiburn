import { describe, expect, it } from "vitest"

import { modelRunName, modelRunNames, modelRunShortNames, modelShortName } from "./models"

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
})
