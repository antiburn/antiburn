import { describe, expect, it } from "vitest"

import {
  environmentKey,
  localRepositoryKey,
  localSessionKey,
  NATIVE_ENVIRONMENT_KEY,
  sameEnvironment,
  sameLocalSession,
  sessionIdentityKey,
} from "./localIdentity"

describe("environmentKey", () => {
  it("names the machine itself when there is no distribution", () => {
    expect(environmentKey()).toBe(NATIVE_ENVIRONMENT_KEY)
    expect(environmentKey(null)).toBe(NATIVE_ENVIRONMENT_KEY)
    expect(environmentKey("")).toBe(NATIVE_ENVIRONMENT_KEY)
    expect(environmentKey("   ")).toBe(NATIVE_ENVIRONMENT_KEY)
  })

  it("normalizes distribution-name case but keeps distinct distributions apart", () => {
    expect(environmentKey("Ubuntu")).toBe(environmentKey("UBUNTU"))
    expect(environmentKey("Ubuntu")).toBe(environmentKey("ubuntu"))
    expect(environmentKey("Ubuntu")).not.toBe(environmentKey("Debian"))
    expect(environmentKey("Ubuntu-24.04")).not.toBe(environmentKey("Ubuntu-22.04"))
  })

  it("never collides a WSL distribution with the native environment", () => {
    expect(environmentKey("native")).not.toBe(NATIVE_ENVIRONMENT_KEY)
  })

  it("compares two environments through the same normalization", () => {
    expect(sameEnvironment("Ubuntu", "ubuntu")).toBe(true)
    expect(sameEnvironment(undefined, null)).toBe(true)
    expect(sameEnvironment("Ubuntu", undefined)).toBe(false)
  })
})

describe("localSessionKey", () => {
  it("separates the same session id across agents and environments", () => {
    const keys = [
      localSessionKey("claude-code", "same"),
      localSessionKey("codex", "same"),
      localSessionKey("claude-code", "same", "Ubuntu"),
      localSessionKey("claude-code", "same", "Debian"),
      localSessionKey("claude-code", "other"),
    ]
    expect(new Set(keys).size).toBe(keys.length)
  })

  it("treats one distribution under different casings as one session", () => {
    expect(localSessionKey("claude-code", "same", "UBUNTU")).toBe(
      localSessionKey("claude-code", "same", "Ubuntu"),
    )
  })

  it("cannot be forged by a separator character inside a part", () => {
    // A naive `join('|')` would make these two the same string.
    expect(localSessionKey("claude|code", "x")).not.toBe(localSessionKey("claude", "code|x"))
  })

  it("agrees with the identity-record form", () => {
    expect(sessionIdentityKey({ agent: "codex", sessionId: "s1", wslDistro: "Ubuntu" })).toBe(
      localSessionKey("codex", "s1", "Ubuntu"),
    )
    expect(
      sameLocalSession(
        { agent: "codex", sessionId: "s1", wslDistro: "Ubuntu" },
        { agent: "codex", sessionId: "s1", wslDistro: "ubuntu" },
      ),
    ).toBe(true)
    expect(
      sameLocalSession(
        { agent: "codex", sessionId: "s1" },
        { agent: "codex", sessionId: "s2" },
      ),
    ).toBe(false)
  })
})

describe("localRepositoryKey", () => {
  it("prefers a stable key from discovery over the name", () => {
    expect(localRepositoryKey("avery/widgets", "Ubuntu", "stable-wsl-clone")).toBe(
      localRepositoryKey("someone/else", null, "stable-wsl-clone"),
    )
  })

  it("scopes a bare name to its environment", () => {
    const native = localRepositoryKey("avery/widgets")
    const wsl = localRepositoryKey("avery/widgets", "Ubuntu")
    expect(native).not.toBe(wsl)
    expect(localRepositoryKey("avery/widgets", "ubuntu")).toBe(wsl)
  })

  it("ignores a blank stable key rather than collapsing every repository onto it", () => {
    expect(localRepositoryKey("avery/widgets", null, "  ")).toBe(
      localRepositoryKey("avery/widgets"),
    )
    expect(localRepositoryKey("avery/widgets", null, "")).not.toBe(
      localRepositoryKey("other/repo", null, ""),
    )
  })

  it("cannot collide a stable key with a name that happens to match it", () => {
    expect(localRepositoryKey("x", null, "native")).not.toBe(localRepositoryKey("native"))
  })
})
