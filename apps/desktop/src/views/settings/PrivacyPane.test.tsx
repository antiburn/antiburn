import { fireEvent, render, screen, waitFor } from "@testing-library/react"
import { beforeEach, expect, it, vi } from "vitest"

import { DEFAULT_SETTINGS } from "../../lib/ipc"
import type * as Ipc from "../../lib/ipc"
import { PrivacyPane } from "./PrivacyPane"

const mocks = vi.hoisted(() => ({ confirm: vi.fn(), clearLocalIndex: vi.fn() }))
vi.mock("@tauri-apps/plugin-dialog", () => ({ confirm: mocks.confirm, save: vi.fn() }))
vi.mock("../../lib/ipc", async (importOriginal) => ({
  ...(await importOriginal<typeof Ipc>()),
  clearLocalIndex: mocks.clearLocalIndex,
}))

beforeEach(() => {
  mocks.confirm.mockReset()
  mocks.clearLocalIndex.mockReset().mockResolvedValue(2)
})

it.each([false, true])(
  "explains local data removal before clear-index approval=%s",
  async (approved) => {
    mocks.confirm.mockResolvedValue(approved)
    const { container } = render(
      <PrivacyPane settings={DEFAULT_SETTINGS} loaded update={vi.fn()} info={null} />,
    )
    expect(container).toHaveTextContent(
      "Preferences, scan folders, and repository choices are kept",
    )
    fireEvent.click(screen.getByRole("button", { name: "Clear index…" }))
    await waitFor(() => expect(mocks.confirm).toHaveBeenCalledOnce())
    const [message] = mocks.confirm.mock.calls[0] as [string]
    expect(message).toContain("Original transcript files are unchanged")
    expect(message).toContain("Available local transcripts can be rediscovered on a later scan")
    expect(message).not.toMatch(/SSH/i)
    await waitFor(() => expect(mocks.clearLocalIndex).toHaveBeenCalledTimes(approved ? 1 : 0))
  },
)

it("explains selected provider routing without changing saved settings", () => {
  const update = vi.fn()
  const { container } = render(
    <PrivacyPane settings={DEFAULT_SETTINGS} loaded update={update} info={null} />,
  )

  expect(container).toHaveTextContent(
    "the provider selected under Model-based checks: Jev, compatible Ollama, Cloudflare, or Custom",
  )
  fireEvent.click(screen.getByRole("button", { name: "When your work can leave this machine" }))
  expect(container).toHaveTextContent(
    "A local or remote endpoint determines where the request goes",
  )
  expect(container).toHaveTextContent(
    "keeps saved connections and credentials but stops new checks",
  )
  expect(container).toHaveTextContent(
    "selected public messages, tool data, response-cycle or session facts, and current instruction/skill inventories",
  )
  expect(container).toHaveTextContent(
    "The check definition selects the inputs, subject to source availability",
  )
  expect(container).toHaveTextContent(
    "Skill inventory inputs contain descriptions, not full skill bodies",
  )
  expect(container).toHaveTextContent("Selected tool results can still contain file text")
  expect(container).toHaveTextContent(
    "Session deletion keeps these already-incurred totals; Clear index below removes them.",
  )
  expect(container).not.toHaveTextContent("Decision model")
  expect(container).not.toHaveTextContent("Clear Local Data")
  expect(container).not.toHaveTextContent("user-slot text to Jev using your API key")
  expect(update).not.toHaveBeenCalled()
})
