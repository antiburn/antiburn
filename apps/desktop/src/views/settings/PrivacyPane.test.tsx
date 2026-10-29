import { fireEvent, render, screen } from "@testing-library/react"
import { expect, it, vi } from "vitest"

import { DEFAULT_SETTINGS } from "../../lib/ipc"
import { PrivacyPane } from "./PrivacyPane"

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
