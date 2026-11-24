import { render, screen } from "@testing-library/react"
import { describe, expect, it } from "vitest"
import type { CloudOngoingStatus } from "../../lib/cloud"
import { ongoingStatus } from "../../onboarding/__tests__/cloudProjectFixtures"
import { CloudSyncActivity, uploadProgressMessage } from "./CloudSyncActivity"

const progress = {
  uploaded: 1,
  total: 5,
  transfer: { sentBytes: 12_000_000, totalBytes: 85_000_000 },
} satisfies NonNullable<CloudOngoingStatus["uploadProgress"]>

describe("uploadProgressMessage", () => {
  it("shows accepted sessions and cumulative transferred bytes", () => {
    expect(uploadProgressMessage([ongoingStatus({ uploadProgress: progress })])).toBe(
      "1/5 sessions, 12 MB of 85 MB uploaded",
    )
  })

  it.each(["Checking folders", "Checking saved uploads"])(
    "keeps counts and bytes visible while %s",
    (phase) => {
      expect(uploadProgressMessage([ongoingStatus({ uploadProgress: progress })], phase)).toBe(
        `1/5 sessions, 12 MB of 85 MB uploaded • ${phase}`,
      )
    },
  )

  it("marks the session count as unfinished until discovery completes", () => {
    expect(
      uploadProgressMessage([
        ongoingStatus({ uploadProgress: { ...progress, total: null, found: 5 } }),
      ]),
    ).toBe("1/5+ sessions, 12 MB of 85 MB uploaded")
  })

  it("shows an unknown session total before discovery finds a session", () => {
    expect(
      uploadProgressMessage([
        ongoingStatus({ uploadProgress: { ...progress, uploaded: 0, total: null } }),
      ]),
    ).toBe("0/? sessions, 12 MB of 85 MB uploaded")
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
    ).toBe("1/5 sessions, 12 MB of ? uploaded")
  })

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
    ).toBe("0/0 sessions, 0 B of 0 B uploaded")
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
    ).toBe("1/5 sessions, 0 B of 85 MB uploaded")
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
    ).toBe("5/5 sessions, 85 MB of 85 MB uploaded • Checking folders")
  })

  it.each([
    { sentBytes: 85_000_000, totalBytes: 85_000_000 },
    { sentBytes: 0, totalBytes: 1_000_000 },
  ])("ignores a per-upload transfer reset to $sentBytes bytes", (uploadTransfer) => {
    expect(
      uploadProgressMessage([ongoingStatus({ uploadProgress: progress, uploadTransfer })]),
    ).toBe("1/5 sessions, 12 MB of 85 MB uploaded")
  })

  it("does not use per-upload bytes when cumulative transfer is missing", () => {
    expect(
      uploadProgressMessage([
        ongoingStatus({
          uploadProgress: { uploaded: 1, total: 5 },
          uploadTransfer: { sentBytes: 12_000_000, totalBytes: 85_000_000 },
        }),
      ]),
    ).toBe("1/5 sessions, ? of ? uploaded")
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
    ).toBe("3/8 session uploads, 19 MB of 105 MB uploaded")
  })

  it("keeps the combined session total unfinished if any scan has gaps", () => {
    expect(
      uploadProgressMessage([
        ongoingStatus({ uploadProgress: progress }),
        ongoingStatus({
          destinationId: "synthetic-team",
          uploadProgress: { ...progress, scanIncomplete: true },
        }),
      ]),
    ).toBe("2/10+ session uploads, 24 MB of 170 MB uploaded")
  })

  it("keeps combined totals unknown if a workspace has no progress", () => {
    expect(
      uploadProgressMessage([
        ongoingStatus({ uploadProgress: progress }),
        ongoingStatus({ destinationId: "synthetic-team" }),
      ]),
    ).toBe("?/5+ session uploads, ? of ? uploaded")
  })

  it.each([
    {
      name: "a missing transfer",
      otherProgress: { uploaded: 2, total: 3 },
      expected: "3/8 session uploads, ? of ? uploaded",
    },
    {
      name: "an unknown transfer total",
      otherProgress: {
        uploaded: 2,
        total: 3,
        transfer: { sentBytes: 0, totalBytes: null },
      },
      expected: "3/8 session uploads, 12 MB of ? uploaded",
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
            total: 5_678,
            transfer: { sentBytes: 1_250_000_000, totalBytes: 8_500_000_000 },
          },
        }),
      ]),
    ).toBe(
      `${countFormat.format(1_234)}/${countFormat.format(5_678)} sessions, ${sizeFormat.format(1.25)} GB of ${sizeFormat.format(8.5)} GB uploaded`,
    )
  })

  it.each([
    { policies: [] },
    { policies: [ongoingStatus()] },
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
      expect(uploadProgressMessage(policies, "Starting sync")).toBe(
        "?/? sessions, ? of ? uploaded • Starting sync",
      )
    },
  )
})

describe("CloudSyncActivity", () => {
  it("keeps one live region and text node through progress, recovery, and errors", () => {
    const initialMessage = "?/? sessions, ? of ? uploaded • Starting sync"
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
        message: "2/5 sessions, 24 MB of 85 MB uploaded • Checking saved uploads",
        active: true,
      },
      { message: "Recovering saved uploads…", active: true },
      { message: "Could not upload sessions. Try again.", active: false },
      { message: "5/5 sessions, 85 MB of 85 MB uploaded", active: false },
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
    const message = "123/456 session uploads, 12 MB of 85 MB uploaded • Checking saved uploads"
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
