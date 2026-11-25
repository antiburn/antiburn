import { render, screen } from "@testing-library/react"
import { describe, expect, it } from "vitest"
import type { CloudOngoingStatus } from "../../lib/cloud"
import { ongoingStatus } from "../../onboarding/__tests__/cloudProjectFixtures"
import {
  CloudSyncActivity,
  currentUploadProgressMessage,
  savedUploadProgressMessage,
  uploadProgressMessage,
} from "./CloudSyncActivity"

const progress = {
  uploaded: 1,
  total: 5,
  transfer: { sentBytes: 12_000_000, totalBytes: 85_000_000 },
} satisfies NonNullable<CloudOngoingStatus["uploadProgress"]>

describe("savedUploadProgressMessage", () => {
  it("separates a single upload's bytes from batch progress", () => {
    expect(
      savedUploadProgressMessage([
        ongoingStatus({
          queuedCount: 1,
          uploadTransfer: { sentBytes: 12_000_000, totalBytes: 85_000_000 },
        }),
      ]),
    ).toEqual(["1 saved upload pending", "Current upload: 12 MB of 85 MB transferred"])
  })

  it("counts pending jobs across workspaces without implying a total", () => {
    expect(
      savedUploadProgressMessage([
        ongoingStatus({ queuedCount: 2, acceptedCount: 12 }),
        ongoingStatus({ queuedCount: 1, acceptedCount: 35 }),
      ]),
    ).toEqual(["3 saved uploads pending"])
    expect(savedUploadProgressMessage([ongoingStatus({ acceptedCount: 12 })])).toEqual([])
  })
})

describe("currentUploadProgressMessage", () => {
  const background = ongoingStatus({
    backgroundSyncInProgress: true,
    uploadProgress: progress,
    backgroundProgress: {
      uploaded: 2,
      total: null,
      transfer: { sentBytes: 30_000_000, totalBytes: null },
    },
  })

  it("shows current background counters instead of the last explicit scan", () => {
    expect(currentUploadProgressMessage([background])).toBe(
      "2 sessions uploaded (30 MB transferred)",
    )
  })

  it("does not substitute historical counts when background progress is unavailable", () => {
    const { backgroundProgress: _progress, ...pending } = background
    expect(currentUploadProgressMessage([pending])).toBeNull()
  })

  it("keeps explicit scan counters visible through post-scan acceptance", () => {
    const explicit = ongoingStatus({ fullSyncInProgress: true, uploadProgress: progress })
    expect(currentUploadProgressMessage([explicit, background])).toBe(
      "Syncing 1 of 5 sessions (12 MB transferred)",
    )
  })

  it("keeps the found count visible when every session found so far has been accepted", () => {
    expect(
      currentUploadProgressMessage([
        { ...background, backgroundProgress: { uploaded: 4, found: 4, total: null } },
      ]),
    ).toBe("Syncing 4 of 4 sessions found")
  })

  it("does not show a previous full scan as progress during saved-upload recovery", () => {
    expect(
      currentUploadProgressMessage([
        ongoingStatus({
          status: "syncing",
          automaticRecoveryCount: 1,
          uploadProgress: progress,
        }),
      ]),
    ).toBeNull()
  })

  it("does not say there is nothing to upload while saved work remains", () => {
    expect(
      currentUploadProgressMessage([
        {
          ...background,
          queuedCount: 1,
          backgroundProgress: { uploaded: 0, total: 0 },
        },
      ]),
    ).toBeNull()
  })
})

describe("uploadProgressMessage", () => {
  it("shows accepted sessions and cumulative transferred bytes", () => {
    expect(uploadProgressMessage([ongoingStatus({ uploadProgress: progress })])).toBe(
      "1 of 5 sessions uploaded • 12 MB of 85 MB transferred",
    )
  })

  it.each(["Checking folders", "Checking saved uploads"])(
    "keeps counts and bytes visible while %s",
    (phase) => {
      expect(uploadProgressMessage([ongoingStatus({ uploadProgress: progress })], phase)).toBe(
        `1 of 5 sessions uploaded • 12 MB of 85 MB transferred • ${phase}`,
      )
    },
  )

  it("shows sessions found without using them as an unfinished denominator", () => {
    expect(
      uploadProgressMessage([
        ongoingStatus({ uploadProgress: { ...progress, total: null, found: 5 } }),
      ]),
    ).toBe("1 session uploaded • 5 sessions found • 12 MB of 85 MB transferred")
  })

  it("keeps an observed zero acceptance count when the session total is unknown", () => {
    expect(
      uploadProgressMessage([
        ongoingStatus({ uploadProgress: { ...progress, uploaded: 0, total: null } }),
      ]),
    ).toBe("0 sessions uploaded • 12 MB of 85 MB transferred")
  })

  it("does not replace an unknown byte total with zero", () => {
    expect(
      uploadProgressMessage([
        ongoingStatus({
          uploadProgress: {
            ...progress,
            transfer: { sentBytes: 12_000_000, totalBytes: null },
          },
        }),
      ]),
    ).toBe("1 of 5 sessions uploaded • 12 MB transferred")
  })

  it.each([
    { uploaded: 0, total: 1, expected: "0 of 1 session uploaded" },
    { uploaded: 1, total: 1, expected: "1 of 1 session uploaded" },
    { uploaded: 1, total: null, expected: "1 session uploaded" },
    { uploaded: 2, total: null, expected: "2 sessions uploaded" },
  ])("uses the measured count with correct plurals: $expected", ({ expected, ...value }) => {
    expect(uploadProgressMessage([ongoingStatus({ uploadProgress: value })])).toBe(expected)
  })

  it.each([
    {
      value: { uploaded: 1, total: 5, scanIncomplete: true, found: 5 },
      expected: "1 session uploaded • 5 sessions found",
    },
    {
      value: { uploaded: 1, total: 5, scanIncomplete: true },
      expected: "1 session uploaded",
    },
    {
      value: { uploaded: 0, total: 0, scanIncomplete: true },
      expected: "0 sessions uploaded",
    },
    {
      value: { uploaded: 1, total: null, found: 1 },
      expected: "1 session uploaded",
    },
    {
      value: { uploaded: 1, total: 1, found: 5 },
      expected: "1 of 1 session uploaded",
    },
  ])(
    "does not turn incomplete or stale discovery into a final total: $expected",
    ({ value, expected }) => {
      expect(uploadProgressMessage([ongoingStatus({ uploadProgress: value })])).toBe(expected)
    },
  )

  it("keeps zero totals when an empty scan is complete", () => {
    expect(
      uploadProgressMessage([
        ongoingStatus({
          uploadProgress: {
            uploaded: 0,
            total: 0,
            transfer: { sentBytes: 0, totalBytes: 0 },
          },
        }),
      ]),
    ).toBe("No sessions to upload")
  })

  it("shows zero sent bytes when cumulative transfer proves no bytes were sent", () => {
    expect(
      uploadProgressMessage([
        ongoingStatus({
          uploadProgress: {
            ...progress,
            transfer: { sentBytes: 0, totalBytes: 85_000_000 },
          },
        }),
      ]),
    ).toBe("1 of 5 sessions uploaded • 0 B of 85 MB transferred")
  })

  it("keeps measured zero bytes when the transfer total is unknown", () => {
    expect(
      uploadProgressMessage([
        ongoingStatus({
          uploadProgress: {
            ...progress,
            transfer: { sentBytes: 0, totalBytes: null },
          },
        }),
      ]),
    ).toBe("1 of 5 sessions uploaded • 0 B transferred")
  })

  it("omits zero-byte transfer work for sessions with matching acceptance receipts", () => {
    expect(
      uploadProgressMessage([
        ongoingStatus({
          uploadProgress: {
            uploaded: 5,
            total: 5,
            transfer: { sentBytes: 0, totalBytes: 0 },
          },
        }),
      ]),
    ).toBe("5 of 5 sessions uploaded")
  })

  it("does not use completed byte transfer to claim session acceptance", () => {
    expect(
      uploadProgressMessage([
        ongoingStatus({
          uploadProgress: {
            uploaded: 0,
            total: 5,
            transfer: { sentBytes: 85_000_000, totalBytes: 85_000_000 },
          },
        }),
      ]),
    ).toBe("0 of 5 sessions uploaded • 85 MB of 85 MB transferred")
  })

  it.each([
    { sentBytes: 1, totalBytes: 999, expected: "1 B of 999 B transferred" },
    { sentBytes: 1_000, totalBytes: 2_000, expected: "1 KB of 2 KB transferred" },
    { sentBytes: 12_000_000, totalBytes: 85_000_000, expected: "12 MB of 85 MB transferred" },
    {
      sentBytes: 1_000_000_000,
      totalBytes: 2_000_000_000,
      expected: "1 GB of 2 GB transferred",
    },
  ])("uses decimal byte units: $expected", ({ expected, ...transfer }) => {
    expect(
      uploadProgressMessage([
        ongoingStatus({ uploadProgress: { uploaded: 1, total: 5, transfer } }),
      ]),
    ).toBe(`1 of 5 sessions uploaded • ${expected}`)
  })

  it("keeps completed counts and bytes during a later routine phase", () => {
    expect(
      uploadProgressMessage(
        [
          ongoingStatus({
            status: "ready",
            uploadProgress: {
              uploaded: 5,
              total: 5,
              transfer: { sentBytes: 85_000_000, totalBytes: 85_000_000 },
            },
          }),
        ],
        "Checking folders",
      ),
    ).toBe("5 of 5 sessions uploaded • 85 MB of 85 MB transferred • Checking folders")
  })

  it.each([
    { sentBytes: 85_000_000, totalBytes: 85_000_000 },
    { sentBytes: 0, totalBytes: 1_000_000 },
  ])("ignores a per-upload transfer reset to $sentBytes bytes", (uploadTransfer) => {
    expect(
      uploadProgressMessage([ongoingStatus({ uploadProgress: progress, uploadTransfer })]),
    ).toBe("1 of 5 sessions uploaded • 12 MB of 85 MB transferred")
  })

  it("does not use per-upload bytes when cumulative transfer is missing", () => {
    expect(
      uploadProgressMessage([
        ongoingStatus({
          uploadProgress: { uploaded: 1, total: 5 },
          uploadTransfer: { sentBytes: 12_000_000, totalBytes: 85_000_000 },
        }),
      ]),
    ).toBe("1 of 5 sessions uploaded")
  })

  it("counts separate workspace deliveries as session uploads", () => {
    expect(
      uploadProgressMessage([
        ongoingStatus({ uploadProgress: progress }),
        ongoingStatus({
          destinationId: "synthetic-team",
          uploadProgress: {
            uploaded: 2,
            total: 3,
            transfer: { sentBytes: 7_000_000, totalBytes: 20_000_000 },
          },
        }),
      ]),
    ).toBe("3 of 8 session uploads complete • 19 MB of 105 MB transferred")
  })

  it("keeps the combined session total unfinished if any scan has gaps", () => {
    expect(
      uploadProgressMessage([
        ongoingStatus({ uploadProgress: progress }),
        ongoingStatus({
          destinationId: "synthetic-team",
          uploadProgress: { ...progress, scanIncomplete: true, found: 5 },
        }),
      ]),
    ).toBe(
      "2 session uploads complete • 10 session uploads found • 24 MB of 170 MB transferred",
    )
  })

  it("omits partial combined counts and bytes if a workspace has no progress", () => {
    expect(
      uploadProgressMessage([
        ongoingStatus({ uploadProgress: progress }),
        ongoingStatus({ destinationId: "synthetic-team" }),
      ]),
    ).toBeNull()
  })

  it("shows only the phase when one workspace has no measurements", () => {
    expect(
      uploadProgressMessage(
        [
          ongoingStatus({ uploadProgress: progress }),
          ongoingStatus({ destinationId: "synthetic-team" }),
        ],
        "Checking folders",
      ),
    ).toBe("Checking folders")
  })

  it("uses a finalized workspace total instead of its stale found count in a combined lower bound", () => {
    expect(
      uploadProgressMessage([
        ongoingStatus({ uploadProgress: { uploaded: 1, total: 5, found: 99 } }),
        ongoingStatus({
          destinationId: "synthetic-team",
          uploadProgress: { uploaded: 2, total: null, found: 3 },
        }),
      ]),
    ).toBe("3 session uploads complete • 8 session uploads found")
  })

  it.each([
    { total: 0, expected: "1 of 1 session upload complete" },
    { total: null, expected: "1 session upload complete" },
  ])("keeps a singular combined delivery count: $expected", ({ total, expected }) => {
    expect(
      uploadProgressMessage([
        ongoingStatus({ uploadProgress: { uploaded: 1, total: 1 } }),
        ongoingStatus({
          destinationId: "synthetic-team",
          uploadProgress: { uploaded: 0, total },
        }),
      ]),
    ).toBe(expected)
  })

  it.each([
    {
      name: "a missing transfer",
      otherProgress: { uploaded: 2, total: 3 },
      expected: "3 of 8 session uploads complete",
    },
    {
      name: "an unknown transfer total",
      otherProgress: {
        uploaded: 2,
        total: 3,
        transfer: { sentBytes: 0, totalBytes: null },
      },
      expected: "3 of 8 session uploads complete • 12 MB transferred",
    },
  ])("preserves the combined byte evidence with $name", ({ otherProgress, expected }) => {
    expect(
      uploadProgressMessage([
        ongoingStatus({ uploadProgress: progress }),
        ongoingStatus({
          destinationId: "synthetic-team",
          uploadProgress: otherProgress,
        }),
      ]),
    ).toBe(expected)
  })

  it("formats large counts and fractional sizes with the current locale", () => {
    const countFormat = new Intl.NumberFormat()
    const sizeFormat = new Intl.NumberFormat(undefined, { maximumFractionDigits: 1 })
    expect(
      uploadProgressMessage([
        ongoingStatus({
          uploadProgress: {
            uploaded: 1_234,
            total: 4_096,
            transfer: { sentBytes: 1_250_000_000, totalBytes: 8_500_000_000 },
          },
        }),
      ]),
    ).toBe(
      `${countFormat.format(1_234)} of ${countFormat.format(4_096)} sessions uploaded • ${sizeFormat.format(1.25)} GB of ${sizeFormat.format(8.5)} GB transferred`,
    )
  })

  it.each([
    { policies: [] },
    { policies: [ongoingStatus()] },
    {
      policies: [ongoingStatus(), ongoingStatus({ destinationId: "synthetic-team" })],
    },
    {
      policies: [
        ongoingStatus({
          acceptedCount: 5,
          queuedCount: 3,
          uploadTransfer: { sentBytes: 12_000_000, totalBytes: 85_000_000 },
        }),
      ],
    },
  ])("does not invent progress without an observation or phase", ({ policies }) => {
    expect(uploadProgressMessage(policies)).toBeNull()
  })

  it.each([{ policies: [] }, { policies: [ongoingStatus()] }])(
    "shows a phase before progress is available",
    ({ policies }) => {
      expect(uploadProgressMessage(policies, "Starting sync")).toBe("Starting sync")
    },
  )
})

describe("CloudSyncActivity", () => {
  it("keeps one live region and text node through progress, recovery, and errors", () => {
    const initialMessage = "Starting sync"
    const { rerender } = render(<CloudSyncActivity message={initialMessage} active />)
    const status = screen.getByRole("status")
    const messageElement = screen.getByTitle(initialMessage)
    const textNode = messageElement.firstChild

    expect(status).toBeVisible()
    expect(status.textContent).toBe(initialMessage)
    expect(status).toContainElement(messageElement)
    expect(textNode?.nodeType).toBe(Node.TEXT_NODE)
    expect(status.querySelector('[aria-hidden="true"]')).not.toBeNull()

    for (const { message, active } of [
      {
        message:
          "2 of 5 sessions uploaded • 24 MB of 85 MB transferred • Checking saved uploads",
        active: true,
      },
      { message: "Recovering saved uploads…", active: true },
      { message: "Could not upload sessions. Try again.", active: false },
      { message: "5 of 5 sessions uploaded • 85 MB of 85 MB transferred", active: false },
    ]) {
      rerender(<CloudSyncActivity message={message} active={active} />)

      expect(screen.getAllByRole("status")).toHaveLength(1)
      expect(screen.getByRole("status")).toBe(status)
      expect(status.textContent).toBe(message)
      expect(screen.getByTitle(message)).toBe(messageElement)
      expect(messageElement.firstChild).toBe(textNode)
    }

    expect(status.querySelector('[aria-hidden="true"]')).toBeNull()
  })

  it("allows the complete progress message to wrap instead of truncating it", () => {
    const message =
      "123 of 456 session uploads complete • 12 MB of 85 MB transferred • Checking saved uploads"
    render(<CloudSyncActivity message={message} active />)

    const status = screen.getByRole("status")
    const messageElement = screen.getByTitle(message)
    expect(status.textContent).toBe(message)
    expect(messageElement).toHaveClass("wrap-anywhere")
    expect(messageElement).not.toHaveClass("truncate")
    expect(messageElement).not.toHaveClass("whitespace-nowrap")
    expect(status).not.toHaveClass("truncate")
  })
})
