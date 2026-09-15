import { act, cleanup, fireEvent, render, screen } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import type * as ClipboardModule from "../../lib/clipboard"
import type * as IpcModule from "../../lib/ipc"
import type * as HygieneModule from "../../lib/useSessionHygiene"
import type { SessionAnalysisPayload } from "../../lib/ipc"
import type { SessionHygienePayload } from "../../lib/insightsIpc"
import type { LocalSessionIdentity } from "../../lib/types/session"
import { localSessionKey } from "../../lib/presentation/localIdentity"
import { sessionDiscussionPrompt } from "../../lib/presentation/sessionDiscussionPrompt"
import { SessionPane, type SessionPaneProps } from "./SessionPane"

const mocks = vi.hoisted(() => ({
  revealSource: vi.fn(),
  writeClipboardText: vi.fn(),
  hygiene: {
    evidenceState: "stale",
    badges: [
      {
        id: "fastModeOveruse",
        status: "finding",
        notAssessedReason: null,
        findingEvidence: { kind: "fastModeOveruse", delegatedTurns: 4 },
      },
    ],
  } as SessionHygienePayload,
}))

vi.mock("../../lib/clipboard", async (importOriginal) => ({
  ...(await importOriginal<typeof ClipboardModule>()),
  writeClipboardText: mocks.writeClipboardText,
}))

vi.mock("../../lib/ipc", async (importOriginal) => ({
  ...(await importOriginal<typeof IpcModule>()),
  revealSource: mocks.revealSource,
}))

vi.mock("../../lib/useSessionHygiene", async (importOriginal) => ({
  ...(await importOriginal<typeof HygieneModule>()),
  useSessionHygiene: (identities: LocalSessionIdentity[]) =>
    new Map(
      identities.map((identity) => [
        localSessionKey(identity.agent, identity.sessionId, identity.wslDistro),
        mocks.hygiene,
      ]),
    ),
}))

afterEach(cleanup)

/** The smallest analysis payload the pane accepts, with a chosen source path. */
function analysisPayload(sourcePath: string | null): SessionAnalysisPayload {
  return {
    summary: null,
    supportsAnalysis: true,
    title: "Fix the flaky test",
    wslDistro: null,
    isActive: false,
    cost: null,
    topLevelCost: null,
    subagentsCost: null,
    inclusiveTokens: null,
    subagentsTokens: null,
    efficiency: null,
    models: [],
    modelRuns: [],
    orchestration: null,
    relations: null,
    sourcePath,
    startedAtEpoch: null,
    analysisPending: false,
    analysisStale: false,
  }
}

function paneProps(sourcePath: string | null): SessionPaneProps {
  return {
    subject: {
      agent: "claude-code",
      sessionId: "session-1",
      wslDistro: null,
      title: "Fix the flaky test",
    },
    payload: analysisPayload(sourcePath),
    loading: false,
    refreshing: false,
    error: false,
    onOpenSession: () => {},
    onDeleted: () => {},
  }
}

function pane(sourcePath: string | null) {
  return render(<SessionPane {...paneProps(sourcePath)} />)
}

beforeEach(() => {
  vi.clearAllMocks()
  mocks.writeClipboardText.mockResolvedValue(undefined)
})

describe("SessionPane — copy path", () => {
  it.each([
    "/Users/dev/.claude/projects/app/session-1.jsonl",
    "/Users/name with spaces/Éxamples/“quoted” & $PATH/transcript.jsonl",
    String.raw`C:\Users\dev\AppData\Roaming\agent logs\session 1.jsonl`,
    "/tmp/日本語/セッション⚡️.jsonl",
    "/weird/newline\nand\ttab/path.jsonl",
  ])("copies the exact source path as plain text: %s", async (sourcePath) => {
    pane(sourcePath)

    await act(async () => {
      fireEvent.click(screen.getByLabelText("Copy path"))
    })

    expect(mocks.writeClipboardText).toHaveBeenCalledExactlyOnceWith(sourcePath)
    expect(screen.getByTestId("copy-path-tick")).toBeTruthy()
  })

  it.each(["altKey", "ctrlKey", "metaKey"])(
    "copies the exact prompt from loaded analysis and hygiene synchronously on %s",
    async (modifier) => {
      const props = paneProps("/tmp/synthetic/session.jsonl")
      render(<SessionPane {...props} />)
      const expected = sessionDiscussionPrompt({
        subject: props.subject,
        payload: props.payload,
        hygiene: mocks.hygiene,
        loading: props.loading,
        refreshing: props.refreshing,
        error: props.error,
      })
      await act(async () => {
        fireEvent.click(screen.getByLabelText("Copy path"), { [modifier]: true })
        expect(mocks.writeClipboardText).toHaveBeenCalledExactlyOnceWith(expected)
      })
      expect(expected).toContain("Fast mode was used for 4 delegated turns")
      expect(expected).toContain("Burn-check evidence: stale")
      expect(screen.getByRole("status")).toHaveTextContent("Prompt copied")
      expect(mocks.revealSource).not.toHaveBeenCalled()
    },
  )

  it("copies only the path with Shift held", async () => {
    pane("/tmp/synthetic/session.jsonl")
    await act(async () =>
      fireEvent.click(screen.getByLabelText("Copy path"), { shiftKey: true }),
    )
    expect(mocks.writeClipboardText).toHaveBeenCalledExactlyOnceWith(
      "/tmp/synthetic/session.jsonl",
    )
  })

  it("uses new loaded data after a refresh without copying old metrics", async () => {
    const props = paneProps("/tmp/synthetic/session.jsonl")
    const { rerender } = render(<SessionPane {...props} />)
    const updated = { ...props.payload!, title: "Updated title", analysisStale: true }
    rerender(<SessionPane {...props} payload={updated} refreshing />)
    await act(async () => fireEvent.click(screen.getByLabelText("Copy path"), { altKey: true }))
    expect(mocks.writeClipboardText).toHaveBeenCalledExactlyOnceWith(
      sessionDiscussionPrompt({
        subject: props.subject,
        payload: updated,
        hygiene: mocks.hygiene,
        loading: false,
        refreshing: true,
        error: false,
      }),
    )
  })

  it("does not show prompt success when the clipboard write fails", async () => {
    pane("/tmp/synthetic/session.jsonl")
    mocks.writeClipboardText.mockRejectedValue(new Error("denied"))
    await act(async () => fireEvent.click(screen.getByLabelText("Copy path"), { altKey: true }))
    expect(screen.queryByTestId("copy-path-tick")).toBeNull()
  })

  it("hides copy and reveal when the payload has no source path", () => {
    pane(null)
    expect(screen.queryByLabelText("Copy path")).toBeNull()
    expect(screen.queryByLabelText("Reveal in file manager")).toBeNull()
  })

  it("shows no success tick when the clipboard write fails", async () => {
    mocks.writeClipboardText.mockRejectedValue(new Error("denied"))
    pane("/Users/dev/.claude/projects/app/session-1.jsonl")

    await act(async () => {
      fireEvent.click(screen.getByLabelText("Copy path"))
    })

    expect(mocks.writeClipboardText).toHaveBeenCalledOnce()
    expect(screen.queryByTestId("copy-path-tick")).toBeNull()
    expect(screen.getByLabelText("Copy path")).toBeTruthy()
  })

  it("keeps reveal wired to the shell command, separate from copy", async () => {
    const sourcePath = "/Users/dev/.claude/projects/app/session-1.jsonl"
    pane(sourcePath)

    await act(async () => {
      fireEvent.click(screen.getByLabelText("Reveal in file manager"))
    })

    expect(mocks.revealSource).toHaveBeenCalledExactlyOnceWith(sourcePath)
    expect(mocks.writeClipboardText).not.toHaveBeenCalled()
  })
})
