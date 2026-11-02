import { act, fireEvent, render, screen } from "@testing-library/react"
import { StrictMode } from "react"
import { beforeEach, expect, it, vi } from "vitest"
import type { CheckDirectory } from "../../lib/checkDirectories"
import { deferred } from "../main-window/burn-checks/tests/burnChecksTestSupport"
import { CheckFilesSettings } from "./CheckFilesSettings"

const read = vi.hoisted(() => vi.fn())
const open = vi.hoisted(() => vi.fn())
vi.mock("../../lib/checkDirectories", () => ({
  getCheckDirectories: read,
  openCheckDirectory: open,
}))
vi.mock("../../lib/platform", () => ({ detectPlatform: () => "macos" }))

const ready: CheckDirectory = {
  reference: "native-opaque-reference",
  scope: "global",
  label: "This device",
  path: "/fixture/.antiburn/checks",
  state: "ready",
  fileCount: 2,
  message: null,
}

beforeEach(() => {
  read.mockReset().mockResolvedValue([ready])
  open.mockReset().mockResolvedValue(undefined)
})

it("shows only ready folders with safe positive physical file counts", async () => {
  read.mockResolvedValueOnce([
    ready,
    ...[0, -1, 1.5, NaN, Infinity, Number.MAX_SAFE_INTEGER + 1, null].map(
      (fileCount, index) => ({
        ...ready,
        label: `Omitted count ${index}`,
        path: `/fixture/count-${index}`,
        fileCount,
      }),
    ),
    ...(["missing", "paused", "unavailable"] as const).map((state) => ({
      ...ready,
      state,
      label: `Omitted state ${state}`,
      path: `/fixture/${state}`,
      fileCount: 5,
      message: "Unknown folder diagnostic",
    })),
    {
      ...ready,
      label: "No open reference",
      path: "/fixture/other",
      reference: null,
      fileCount: 1,
    },
  ])
  const { container } = render(<CheckFilesSettings control="checkDirectories" />)
  await screen.findByText("2 check files")
  expect(screen.getByText("1 check file")).toBeVisible()
  expect(screen.queryByText(/Omitted/)).not.toBeInTheDocument()
  expect(container).not.toHaveTextContent(/unknown|unavailable|valid definitions|runnable/i)
  expect(screen.getAllByRole("button", { name: /^Open in Finder:/ })).toHaveLength(1)
  fireEvent.click(screen.getByRole("button", { name: /^Open in Finder:/ }))
  expect(open).toHaveBeenCalledExactlyOnceWith("native-opaque-reference")
  expect(screen.queryByRole("button", { name: /refresh|retry/i })).not.toBeInTheDocument()
})

it("does not claim no folders exist when no confirmed check files remain", async () => {
  read.mockResolvedValueOnce([
    { ...ready, fileCount: 0 },
    { ...ready, state: "paused" },
  ])
  render(<CheckFilesSettings control="checkDirectories" />)
  await screen.findByText("No check files found.")
  expect(screen.queryByText(ready.path)).not.toBeInTheDocument()
  expect(screen.queryByRole("button", { name: /^Open in / })).not.toBeInTheDocument()
})

it("loads fresh rows on remount and never shows previous rows or errors", async () => {
  const { unmount } = render(<CheckFilesSettings control="checkDirectories" />)
  await screen.findByText("2 check files")
  unmount()
  const fresh = deferred<CheckDirectory[]>()
  read.mockReturnValueOnce(fresh.promise)
  render(<CheckFilesSettings control="checkDirectories" />)
  expect(screen.getByRole("status")).toHaveTextContent("Loading check folders…")
  expect(screen.queryByText("2 check files")).not.toBeInTheDocument()
  await act(async () => fresh.reject(new Error("private read error")))
  expect(screen.getByRole("alert")).toHaveTextContent(
    "Could not load check folders. Reopen Settings → Checks to try again.",
  )
  expect(screen.queryByRole("status")).not.toBeInTheDocument()
  expect(screen.queryByText("2 check files")).not.toBeInTheDocument()
  expect(screen.queryByText("No check files found.")).not.toBeInTheDocument()
  expect(read).toHaveBeenCalledTimes(2)
})

it("fences the Strict Mode cleanup read and loads once for the active subscription", async () => {
  const first = deferred<CheckDirectory[]>()
  read.mockReturnValueOnce(first.promise)
  render(
    <StrictMode>
      <CheckFilesSettings control="checkDirectories" />
    </StrictMode>,
  )
  expect(read).toHaveBeenCalledOnce()
  expect(screen.getByRole("status")).toHaveTextContent("Loading check folders…")
  await act(async () => first.resolve([{ ...ready, fileCount: 99 }]))
  await screen.findByText("2 check files")
  expect(screen.queryByText("99 check files")).not.toBeInTheDocument()
  expect(read).toHaveBeenCalledTimes(2)
})

it.each(["before", "after"])(
  "keeps an open failure visible when collapsed %s it fails",
  async (timing) => {
    const request = deferred<void>()
    open.mockReturnValueOnce(request.promise)
    render(<CheckFilesSettings />)
    const disclosure = screen.getByRole("button", { name: "Check files" })
    fireEvent.click(disclosure)
    fireEvent.click(await screen.findByRole("button", { name: /^Open in Finder:/ }))
    if (timing === "before") fireEvent.click(disclosure)
    await act(async () => request.reject(new Error("private failure details")))
    if (timing === "after") fireEvent.click(disclosure)
    expect(disclosure).toHaveAttribute("aria-expanded", "false")
    expect(screen.getByRole("alert")).toHaveTextContent(
      "Could not open a check folder. Reopen Settings → Checks to try again.",
    )
    expect(screen.queryByText(/private failure|Refresh folders/i)).not.toBeInTheDocument()
  },
)
