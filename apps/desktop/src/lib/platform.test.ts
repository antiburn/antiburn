import { describe, expect, it } from "vitest"

import { applyPlatformAttribute, detectPlatform } from "./platform"

/** A navigator stub carrying only the fields detection reads. */
function navigatorWith(fields: { userAgent?: string; hintPlatform?: string }): Navigator {
  const stub: Record<string, unknown> = { userAgent: fields.userAgent ?? "" }
  if (fields.hintPlatform !== undefined) {
    stub.userAgentData = { platform: fields.hintPlatform }
  }
  return stub as unknown as Navigator
}

describe("detectPlatform", () => {
  it("prefers the Client Hints platform when the webview reports one", () => {
    expect(
      detectPlatform(navigatorWith({ hintPlatform: "macOS", userAgent: "Mozilla/5.0 (X11)" })),
    ).toBe("macos")
    expect(detectPlatform(navigatorWith({ hintPlatform: "Windows" }))).toBe("windows")
    expect(detectPlatform(navigatorWith({ hintPlatform: "Linux" }))).toBe("linux")
  })

  it("falls back to the user-agent string", () => {
    expect(
      detectPlatform(navigatorWith({ userAgent: "Mozilla/5.0 (Macintosh; Intel Mac OS X)" })),
    ).toBe("macos")
    expect(detectPlatform(navigatorWith({ userAgent: "Mozilla/5.0 (Windows NT 10.0)" }))).toBe(
      "windows",
    )
    expect(
      detectPlatform(navigatorWith({ userAgent: "Mozilla/5.0 (X11; Linux x86_64)" })),
    ).toBe("linux")
  })

  it("reports unknown rather than guessing", () => {
    expect(detectPlatform(navigatorWith({ userAgent: "some-headless-runtime/1.0" }))).toBe(
      "unknown",
    )
  })
})

describe("applyPlatformAttribute", () => {
  it("publishes the platform so CSS can branch on it", () => {
    const root = document.createElement("html")

    applyPlatformAttribute(root, "macos")

    expect(root.getAttribute("data-platform")).toBe("macos")
  })

  it("overwrites a stale value instead of appending", () => {
    const root = document.createElement("html")

    applyPlatformAttribute(root, "linux")
    applyPlatformAttribute(root, "windows")

    expect(root.getAttribute("data-platform")).toBe("windows")
  })
})
