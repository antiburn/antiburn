import { act, fireEvent, render, screen, within } from "@testing-library/react"
import { useState } from "react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { AppSearch } from "./AppSearch"
import type * as CheckAvailabilityModule from "../../lib/checkAvailability"
import {
  emptyCheckAvailability,
  getCheckAvailability,
  onCheckAvailabilityChanged,
  type CheckAvailability,
  type CheckAvailabilityEvent,
} from "../../lib/checkAvailability"

vi.mock("../../lib/checkAvailability", async (importOriginal) => ({
  ...(await importOriginal<typeof CheckAvailabilityModule>()),
  getCheckAvailability: vi.fn(),
  onCheckAvailabilityChanged: vi.fn(),
}))

let availabilityEvent: (event: CheckAvailabilityEvent) => void
const availableChecks = {
  ...emptyCheckAvailability,
  revision: 1,
  configured: true,
  installedChecks: [
    {
      effectiveId: "installed:retry-loop",
      name: "Retry loop",
      aliases: [],
      availability: { status: "available" },
    },
  ],
} satisfies CheckAvailability

function deferred<T>() {
  let resolve!: (value: T) => void
  let reject!: (reason: Error) => void
  const promise = new Promise<T>((done, fail) => {
    resolve = done
    reject = fail
  })
  return { promise, resolve, reject }
}

beforeEach(() => {
  vi.mocked(getCheckAvailability).mockReset().mockResolvedValue(emptyCheckAvailability)
  vi.mocked(onCheckAvailabilityChanged)
    .mockReset()
    .mockImplementation(async (callback) => {
      availabilityEvent = callback
      return vi.fn<() => void>()
    })
  Object.defineProperty(HTMLDialogElement.prototype, "showModal", {
    configurable: true,
    value: function (this: HTMLDialogElement) {
      this.open = true
    },
  })
  Object.defineProperty(HTMLDialogElement.prototype, "close", {
    configurable: true,
    value: function (this: HTMLDialogElement) {
      this.open = false
    },
  })
  HTMLElement.prototype.scrollIntoView = vi.fn()
})
afterEach(() => vi.restoreAllMocks())
function Harness({
  choose = async () => {},
}: {
  choose?: Parameters<typeof AppSearch>[0]["onChoose"]
}) {
  const [open, setOpen] = useState(false)
  return (
    <>
      <button data-app-search-trigger="titlebar" onClick={() => setOpen(true)}>
        Search
      </button>
      {open && <AppSearch onChoose={choose} onClose={() => setOpen(false)} />}
    </>
  )
}
function open(choose?: Parameters<typeof AppSearch>[0]["onChoose"]) {
  render(<Harness {...(choose ? { choose } : {})} />)
  fireEvent.click(screen.getByRole("button", { name: "Search" }))
  return screen.getByRole("combobox")
}

describe("app search palette", () => {
  it("focuses input, labels grouped results, and closes back to the visible trigger", () => {
    const input = open()
    expect(input).toHaveFocus()
    const list = screen.getByRole("listbox")
    expect(within(list).getByRole("group", { name: "Features" })).toBeInTheDocument()
    expect(within(list).getByRole("group", { name: "Settings" })).toBeInTheDocument()
    fireEvent.click(screen.getByRole("button", { name: "Close search" }))
    expect(screen.queryByRole("dialog")).toBeNull()
    expect(screen.getByRole("button", { name: "Search" })).toHaveFocus()
  })
  it("supports arrows and Enter without invoking a setting mutation", async () => {
    const choose = vi.fn().mockResolvedValue(undefined)
    const input = open(choose)
    fireEvent.change(input, { target: { value: "sound" } })
    expect(screen.getAllByRole("option")).toHaveLength(1)
    fireEvent.keyDown(input, { key: "ArrowDown" })
    await act(async () => fireEvent.keyDown(input, { key: "Enter" }))
    expect(choose).toHaveBeenCalledOnce()
    expect(choose.mock.calls[0]![0].target).toEqual({
      kind: "setting",
      control: "sound",
    })
    expect(screen.queryByRole("dialog")).toBeNull()
  })
  it("does not choose during composition, and announces no results", () => {
    const choose = vi.fn()
    const input = open(choose)
    fireEvent.keyDown(input, { key: "Enter", isComposing: true })
    expect(choose).not.toHaveBeenCalled()
    fireEvent.change(input, { target: { value: "no matching result xyz" } })
    expect(screen.getByRole("status")).toHaveTextContent("No matching destinations")
    expect(input).not.toHaveAttribute("aria-activedescendant")
    fireEvent.keyDown(input, { key: "ArrowDown" })
    fireEvent.keyDown(input, { key: "Enter" })
    expect(choose).not.toHaveBeenCalled()
  })
  it("reopens after failed navigation and permits one explicit retry", async () => {
    const choose = vi
      .fn()
      .mockRejectedValueOnce(new Error("unavailable"))
      .mockResolvedValueOnce(undefined)
    const input = open(choose)
    fireEvent.change(input, { target: { value: "sound" } })
    await act(async () => fireEvent.keyDown(input, { key: "Enter" }))
    expect(screen.getByRole("alert")).toHaveTextContent("Could not open")
    expect(input).toHaveFocus()
    await act(async () => fireEvent.keyDown(input, { key: "Enter" }))
    expect(choose).toHaveBeenCalledTimes(2)
  })
  it("opens a step setting as an ordinary Settings control", async () => {
    const choose = vi.fn().mockResolvedValue(undefined)
    const input = open(choose)
    fireEvent.change(input, { target: { value: "scan folders" } })
    expect(screen.getAllByRole("option")).toHaveLength(1)
    fireEvent.keyDown(input, { key: "ArrowDown" })
    await act(async () => fireEvent.keyDown(input, { key: "Enter" }))
    expect(choose.mock.calls[0]![0].target).toEqual({
      kind: "setting",
      control: "sourceFolders",
    })
  })
  it("suppresses concurrent activations", async () => {
    let resolve!: () => void
    const choose = vi.fn(
      () =>
        new Promise<void>((done) => {
          resolve = done
        }),
    )
    const input = open(choose)
    await act(async () => {
      fireEvent.keyDown(input, { key: "Enter" })
      fireEvent.keyDown(input, { key: "Enter" })
    })
    expect(choose).toHaveBeenCalledOnce()
    await act(async () => resolve())
  })

  it("shows an initial availability error and retries without blocking other destinations", async () => {
    vi.mocked(getCheckAvailability)
      .mockRejectedValueOnce(new Error("IPC failed"))
      .mockResolvedValueOnce(availableChecks)
    const input = open()
    expect(await screen.findByRole("alert")).toHaveTextContent("Could not load checks.")
    fireEvent.change(input, { target: { value: "sound" } })
    expect(screen.getByRole("option", { name: /Sound/ })).toBeVisible()
    expect(screen.getByRole("alert")).toBeVisible()
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Retry" })))
    expect(screen.queryByRole("alert")).not.toBeInTheDocument()
    fireEvent.change(input, { target: { value: "Retry loop" } })
    expect(screen.getByRole("option", { name: /Retry loop/ })).toBeVisible()
  })

  it("retains good destinations after failed events and failed retries, then recovers", async () => {
    vi.mocked(getCheckAvailability)
      .mockResolvedValueOnce(availableChecks)
      .mockRejectedValueOnce(new Error("refresh failed"))
    const input = open()
    fireEvent.change(input, { target: { value: "Retry loop" } })
    expect(await screen.findByRole("option", { name: /Retry loop/ })).toBeVisible()
    act(() => availabilityEvent({ status: "failed" }))
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Retry" })))
    expect(screen.getByRole("alert")).toHaveTextContent("Could not load checks.")
    expect(screen.getByRole("option", { name: /Retry loop/ })).toBeVisible()
    act(() => availabilityEvent({ status: "updated", snapshot: availableChecks }))
    expect(screen.queryByRole("alert")).not.toBeInTheDocument()
  })

  it.each(["resolve", "reject"] as const)(
    "ignores a stale initial request that later %ss after a newer event",
    async (settle) => {
      const request = deferred<CheckAvailability>()
      vi.mocked(getCheckAvailability).mockReturnValueOnce(request.promise)
      const input = open()
      await act(async () => {})
      act(() => availabilityEvent({ status: "updated", snapshot: availableChecks }))
      await act(async () => {
        if (settle === "resolve") request.resolve(emptyCheckAvailability)
        else request.reject(new Error("old failure"))
      })
      fireEvent.change(input, { target: { value: "Retry loop" } })
      expect(screen.getByRole("option", { name: /Retry loop/ })).toBeVisible()
      expect(screen.queryByRole("alert")).not.toBeInTheDocument()
    },
  )

  it("removes a discovered destination on its event and rejects a late old read", async () => {
    const request = deferred<CheckAvailability>()
    vi.mocked(getCheckAvailability).mockReturnValueOnce(request.promise)
    const input = open()
    await act(async () => {})
    act(() => availabilityEvent({ status: "updated", snapshot: availableChecks }))
    fireEvent.change(input, { target: { value: "Retry loop" } })
    expect(screen.getByRole("option", { name: /Retry loop/ })).toBeVisible()

    act(() =>
      availabilityEvent({
        status: "updated",
        snapshot: { ...availableChecks, revision: 2, installedChecks: [] },
      }),
    )
    expect(screen.queryByRole("option", { name: /Retry loop/ })).not.toBeInTheDocument()
    expect(screen.getByRole("status")).toHaveTextContent("No matching destinations")
    await act(async () => request.resolve(availableChecks))
    expect(screen.queryByRole("option", { name: /Retry loop/ })).not.toBeInTheDocument()
    expect(input).toHaveFocus()
    fireEvent.change(input, { target: { value: "Burn checks" } })
    expect(screen.getByRole("option", { name: /ChecksFeatures/ })).toBeVisible()
  })

  it("replaces a project destination with its same-ID global definition", async () => {
    vi.mocked(getCheckAvailability).mockResolvedValueOnce(availableChecks)
    const input = open()
    fireEvent.change(input, { target: { value: "Retry loop" } })
    expect(await screen.findByRole("option", { name: /Retry loop/ })).toBeVisible()
    act(() =>
      availabilityEvent({
        status: "updated",
        snapshot: {
          ...availableChecks,
          revision: 2,
          installedChecks: [{ ...availableChecks.installedChecks[0]!, name: "Global retries" }],
        },
      }),
    )
    expect(screen.queryByRole("option", { name: /Retry loop/ })).not.toBeInTheDocument()
    fireEvent.change(input, { target: { value: "Global retries" } })
    expect(screen.getAllByRole("option")).toHaveLength(1)
    expect(screen.getByRole("option", { name: /Global retries/ })).toBeVisible()
  })

  it("retries a failed subscription while keeping a successful snapshot", async () => {
    vi.mocked(onCheckAvailabilityChanged).mockRejectedValueOnce(new Error("listen failed"))
    vi.mocked(getCheckAvailability).mockResolvedValue(availableChecks)
    const input = open()
    expect(await screen.findByRole("alert")).toHaveTextContent("Could not load checks.")
    fireEvent.change(input, { target: { value: "Retry loop" } })
    expect(screen.getByRole("option", { name: /Retry loop/ })).toBeVisible()
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Retry" })))
    expect(onCheckAvailabilityChanged).toHaveBeenCalledTimes(2)
    expect(screen.queryByRole("alert")).not.toBeInTheDocument()
  })

  it("releases a subscription that finishes registering after unmount", async () => {
    const subscription = deferred<() => void>()
    const stop = vi.fn()
    vi.mocked(onCheckAvailabilityChanged).mockReturnValueOnce(subscription.promise)
    open()
    fireEvent.click(screen.getByRole("button", { name: "Close search" }))
    await act(async () => subscription.resolve(stop))
    expect(stop).toHaveBeenCalledOnce()
    expect(getCheckAvailability).not.toHaveBeenCalled()
  })

  it("keeps a closed palette's response out of the next palette", async () => {
    const request = deferred<CheckAvailability>()
    vi.mocked(getCheckAvailability).mockReturnValueOnce(request.promise)
    open()
    await act(async () => {})
    fireEvent.click(screen.getByRole("button", { name: "Close search" }))
    fireEvent.click(screen.getByRole("button", { name: "Search" }))
    await act(async () => request.resolve(availableChecks))
    fireEvent.change(screen.getByRole("combobox"), { target: { value: "Retry loop" } })
    expect(screen.queryByRole("option", { name: /Retry loop/ })).not.toBeInTheDocument()
    expect(screen.queryByRole("alert")).not.toBeInTheDocument()
  })
})
