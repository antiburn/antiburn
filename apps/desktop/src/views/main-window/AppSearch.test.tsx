import { act, fireEvent, render, screen, within } from "@testing-library/react"
import { useState } from "react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { AppSearch } from "./AppSearch"

beforeEach(() => {
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
  stepSettingsAvailable,
}: {
  choose?: Parameters<typeof AppSearch>[0]["onChoose"]
  stepSettingsAvailable?: boolean
}) {
  const [open, setOpen] = useState(false)
  return (
    <>
      <button data-app-search-trigger="titlebar" onClick={() => setOpen(true)}>
        Search
      </button>
      {open && (
        <AppSearch
          onChoose={choose}
          onClose={() => setOpen(false)}
          {...(stepSettingsAvailable === undefined ? {} : { stepSettingsAvailable })}
        />
      )}
    </>
  )
}
function open(
  choose?: Parameters<typeof AppSearch>[0]["onChoose"],
  stepSettingsAvailable?: boolean,
) {
  render(
    <Harness
      {...(choose ? { choose } : {})}
      {...(stepSettingsAvailable === undefined ? {} : { stepSettingsAvailable })}
    />,
  )
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
  it("surfaces step-settings results, but only once a modal exists to open", async () => {
    const choose = vi.fn().mockResolvedValue(undefined)
    const input = open(choose, true)
    fireEvent.change(input, { target: { value: "scan folders" } })
    expect(screen.getAllByRole("option")).toHaveLength(1)
    fireEvent.keyDown(input, { key: "ArrowDown" })
    await act(async () => fireEvent.keyDown(input, { key: "Enter" }))
    expect(choose.mock.calls[0]![0].target).toEqual({
      kind: "stepSetting",
      control: "sourceFolders",
    })
  })
  it("excludes step-settings results while the first run has no modal to open", () => {
    const input = open(undefined, false)
    fireEvent.change(input, { target: { value: "scan folders" } })
    expect(screen.queryAllByRole("option")).toHaveLength(0)
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
})
