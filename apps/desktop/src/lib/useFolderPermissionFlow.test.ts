import { act, renderHook } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import { requestFolderAccess } from "./ipc"
import { BETWEEN_FOLDERS_MS, useFolderPermissionFlow } from "./useFolderPermissionFlow"
import type { DeferredPermissionDir } from "./types/repository"

vi.mock("./ipc", () => ({ requestFolderAccess: vi.fn() }))

const asked = vi.mocked(requestFolderAccess)

const deferred: DeferredPermissionDir[] = [
  { dir: "Documents", pathCount: 3 },
  { dir: "Desktop", pathCount: 1 },
]

beforeEach(() => {
  vi.useFakeTimers()
  asked.mockReset()
})

afterEach(() => {
  vi.useRealTimers()
})

/**
 * Let pending promises settle while fake timers are installed.
 *
 * `advanceTimersByTimeAsync` rather than `waitFor`: the latter polls on real
 * timers, which never advance here, so it would simply time out.
 */
async function flush(): Promise<void> {
  await act(async () => {
    await vi.advanceTimersByTimeAsync(0)
  })
}

/** Advance past the gap between folders and settle what that starts. */
async function nextFolder(): Promise<void> {
  await act(async () => {
    await vi.advanceTimersByTimeAsync(BETWEEN_FOLDERS_MS)
  })
}

describe("useFolderPermissionFlow", () => {
  it("asks for one folder at a time rather than stacking dialogs", async () => {
    asked.mockResolvedValue({ outcome: "granted", elapsedMs: 900 })
    const { result } = renderHook(() => useFolderPermissionFlow(deferred))

    act(() => result.current.start())
    await flush()

    // The second folder is not asked about until the first has answered and
    // the gap between them has elapsed.
    expect(asked).toHaveBeenCalledTimes(1)
    expect(asked).toHaveBeenCalledWith("Documents")
    expect(result.current.current).toBe("Documents")
    expect(result.current.total).toBe(2)

    await nextFolder()

    expect(asked).toHaveBeenCalledTimes(2)
    expect(asked).toHaveBeenLastCalledWith("Desktop")
    expect(result.current.position).toBe(2)
  })

  it("reports a fast refusal as a recorded denial and stops re-asking for it", async () => {
    asked.mockResolvedValue({ outcome: "recorded-denial", elapsedMs: 2 })
    const { result } = renderHook(() => useFolderPermissionFlow([deferred[0]!]))

    act(() => result.current.start())
    await flush()
    await nextFolder()

    expect(result.current.phase).toBe("done")
    expect(result.current.recordedDenials).toEqual(["Documents"])

    // Starting again is a no-op: the system will not show a dialog, so asking
    // would leave the reader pressing a button that does nothing.
    asked.mockClear()
    act(() => result.current.start())
    await flush()
    expect(asked).not.toHaveBeenCalled()
  })

  it("tracks denials per folder, so one stuck folder does not block the others", async () => {
    asked.mockImplementation(async (dir: string) =>
      dir === "Documents"
        ? { outcome: "recorded-denial" as const, elapsedMs: 1 }
        : { outcome: "granted" as const, elapsedMs: 800 },
    )
    const { result } = renderHook(() => useFolderPermissionFlow(deferred))

    act(() => result.current.start())
    await flush()
    await nextFolder()
    await nextFolder()

    expect(result.current.phase).toBe("done")
    expect(result.current.verdicts).toEqual({
      Documents: "recorded-denial",
      Desktop: "granted",
    })
    expect(result.current.recordedDenials).toEqual(["Documents"])
  })

  it("reports a failed request as an error rather than a denial", async () => {
    asked.mockRejectedValue(new Error("no home directory"))
    const { result } = renderHook(() => useFolderPermissionFlow([deferred[0]!]))

    act(() => result.current.start())
    await flush()

    expect(result.current.verdicts.Documents).toBe("error")
    // An error is not a recorded denial: retrying is reasonable here.
    expect(result.current.recordedDenials).toEqual([])
  })

  it("announces each grant as it lands, not once at the end", async () => {
    asked.mockResolvedValue({ outcome: "granted", elapsedMs: 700 })
    const granted = vi.fn()
    const { result } = renderHook(() => useFolderPermissionFlow(deferred, granted))

    act(() => result.current.start())
    await flush()

    expect(granted).toHaveBeenCalledTimes(1)
    expect(granted).toHaveBeenCalledWith("Documents")
  })

  it("stops asking once the reader navigates away", async () => {
    asked.mockResolvedValue({ outcome: "granted", elapsedMs: 700 })
    const { result, unmount } = renderHook(() => useFolderPermissionFlow(deferred))

    act(() => result.current.start())
    await flush()
    expect(asked).toHaveBeenCalledTimes(1)

    unmount()
    await act(async () => {
      await vi.advanceTimersByTimeAsync(BETWEEN_FOLDERS_MS * 3)
    })

    // No second dialog for a surface nobody is looking at any more.
    expect(asked).toHaveBeenCalledTimes(1)
  })
})
