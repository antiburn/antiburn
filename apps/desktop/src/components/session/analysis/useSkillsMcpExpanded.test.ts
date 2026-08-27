import { act, cleanup, renderHook } from "@testing-library/react"
import { afterEach, describe, expect, it } from "vitest"

import { skillsMcpExpandedStore, useSkillsMcpExpanded } from "./useSkillsMcpExpanded"

afterEach(() => {
  cleanup()
  skillsMcpExpandedStore.set(false)
})

describe("useSkillsMcpExpanded", () => {
  it("starts collapsed", () => {
    const { result } = renderHook(() => useSkillsMcpExpanded())
    expect(result.current[0]).toBe(false)
  })

  it("keeps the flag set after the setting component unmounts", () => {
    const first = renderHook(() => useSkillsMcpExpanded())
    act(() => {
      first.result.current[1](true)
    })
    first.unmount()

    const second = renderHook(() => useSkillsMcpExpanded())
    expect(second.result.current[0]).toBe(true)
  })

  it("shares the flag between components mounted at the same time", () => {
    const a = renderHook(() => useSkillsMcpExpanded())
    const b = renderHook(() => useSkillsMcpExpanded())

    act(() => {
      a.result.current[1](true)
    })

    expect(b.result.current[0]).toBe(true)
  })
})
