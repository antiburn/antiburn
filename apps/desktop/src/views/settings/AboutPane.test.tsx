import { fireEvent, render, screen } from "@testing-library/react"
import { expect, it, vi } from "vitest"

import { DEFAULT_SETTINGS } from "../../lib/ipc"
import { AboutPane } from "./AboutPane"

const documentModuleLoaded = vi.hoisted(() => vi.fn())

vi.mock("./AboutDocumentView", () => {
  documentModuleLoaded()
  return {
    AboutDocumentView: ({ id }: { id: string }) => <h1>{id}</h1>,
  }
})

it("loads the legal document module only after a reader opens a document", async () => {
  render(<AboutPane settings={DEFAULT_SETTINGS} loaded={false} update={vi.fn()} info={null} />)

  expect(documentModuleLoaded).not.toHaveBeenCalled()

  fireEvent.click(screen.getByRole("button", { name: "Open third-party attributions" }))

  expect(await screen.findByRole("heading", { name: "attributions" })).toBeInTheDocument()
  expect(documentModuleLoaded).toHaveBeenCalledOnce()
})
