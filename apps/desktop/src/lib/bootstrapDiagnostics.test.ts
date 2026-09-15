import { describe, expect, it } from "vitest"

import { classifyMainWindowFailure } from "./bootstrapDiagnostics"

describe("bootstrapDiagnostics", () => {
  it("keeps messages, paths, URLs, and rejection values outside reports", () => {
    const secret = new TypeError("token at /Users/example/private https://secret.invalid")
    expect(classifyMainWindowFailure("window_error", secret)).toEqual({
      kind: "window_error",
      category: "type_error",
      errorName: "TypeError",
    })
    expect(
      JSON.stringify(classifyMainWindowFailure("unhandled_rejection", secret)),
    ).not.toContain("secret")
  })

  it("uses the fixed invoke category for responder installation", () => {
    expect(classifyMainWindowFailure("responder_install_failed", new Error("private"))).toEqual(
      {
        kind: "responder_install_failed",
        category: "invoke_error",
        errorName: "Error",
      },
    )
  })

  it("classifies non-errors without serializing their values", () => {
    expect(classifyMainWindowFailure("unhandled_rejection", { token: "private" })).toEqual({
      kind: "unhandled_rejection",
      category: "non_error_value",
      errorName: "Other",
    })
  })

  it("treats throwing property access as unknown", () => {
    const value = Object.defineProperty({}, "name", {
      get() {
        throw new Error("private")
      },
    })
    expect(classifyMainWindowFailure("window_error", value)).toEqual({
      kind: "window_error",
      category: "unknown",
      errorName: "Other",
    })
  })
})
