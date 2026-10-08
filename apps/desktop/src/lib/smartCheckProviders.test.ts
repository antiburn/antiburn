import { expect, it } from "vitest"
import { contextLimitError, defaultConnection, type Connection } from "./smartCheckProviders"

it.each([
  null,
  { totalInputTokens: null, stateAndLongestQuestionTokens: null, runtimeContextTokens: null },
  { totalInputTokens: null, stateAndLongestQuestionTokens: 8192, runtimeContextTokens: null },
  { totalInputTokens: 4096, stateAndLongestQuestionTokens: null, runtimeContextTokens: null },
  { totalInputTokens: 8192, stateAndLongestQuestionTokens: null, runtimeContextTokens: 4096 },
  { totalInputTokens: 8192, stateAndLongestQuestionTokens: 4096, runtimeContextTokens: null },
] satisfies Connection["contextOverride"][])(
  "rejects unusable Custom limits: %j",
  (contextOverride) => {
    expect(
      contextLimitError({ ...defaultConnection("custom"), contextOverride }),
    ).not.toBeNull()
  },
)

it.each([
  { totalInputTokens: 4097, stateAndLongestQuestionTokens: null, runtimeContextTokens: null },
  { totalInputTokens: null, stateAndLongestQuestionTokens: null, runtimeContextTokens: 4097 },
] satisfies Connection["contextOverride"][])(
  "accepts a usable Custom input bound: %j",
  (contextOverride) => {
    expect(contextLimitError({ ...defaultConnection("custom"), contextOverride })).toBeNull()
  },
)

it("keeps Ollama defaults usable and rejects overrides that exhaust the reserve", () => {
  const connection = defaultConnection("ollama")
  expect(contextLimitError(connection)).toBeNull()
  expect(
    contextLimitError({
      ...connection,
      contextOverride: {
        totalInputTokens: null,
        stateAndLongestQuestionTokens: null,
        runtimeContextTokens: 1024,
      },
    }),
  ).toContain("1,024-token rendering reserve")
})
