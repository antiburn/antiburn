import { describe, expect, it } from "vitest"
import type { CloudOngoingStatus } from "../../lib/cloud"
import { ongoingStatus } from "../../onboarding/__tests__/cloudProjectFixtures"
import {
  idleSyncMessage,
  onlyAutomaticRecovery,
  savedUploadDisplayState,
} from "./CloudDestinationSyncStatus"

function enabledPolicy(patch: Partial<CloudOngoingStatus> = {}) {
  return ongoingStatus({
    status: "ready",
    consentVersion: "desktop-project-sync-v1",
    projects: [
      {
        sourceKind: "claude_jsonl",
        sourcePath: "/Example/claude/projects/studio",
        projectPath: "/Example/Studio",
      },
    ],
    ...patch,
  })
}

const idleOptions = {
  deviceEnabled: true,
  workspaceUnavailable: false,
  hasError: false,
}

const uploadProgress = {
  uploaded: 1,
  total: 5,
  transfer: { sentBytes: 12_000_000, totalBytes: 85_000_000 },
}

describe("idleSyncMessage", () => {
  it("keeps measured progress without adding a completion or activity claim", () => {
    expect(
      idleSyncMessage({
        ...idleOptions,
        policies: [enabledPolicy({ uploadProgress })],
      }),
    ).toBe("Last full scan • 1 of 5 sessions uploaded • 12 MB of 85 MB transferred")
  })

  it.each([
    {},
    { acceptedCount: 5, queuedCount: 3 },
    { uploadTransfer: { sentBytes: 12_000_000, totalBytes: 85_000_000 } },
  ])("does not infer running or zero progress from missing scan measurements", (patch) => {
    expect(idleSyncMessage({ ...idleOptions, policies: [enabledPolicy(patch)] })).toBe(
      "Folder sync enabled",
    )
  })

  it("does not show a partial workspace sum when another workspace has no progress", () => {
    expect(
      idleSyncMessage({
        ...idleOptions,
        policies: [
          enabledPolicy({ uploadProgress }),
          enabledPolicy({ destinationId: "synthetic-team" }),
        ],
      }),
    ).toBe("Folder sync enabled")
  })

  it("keeps storage unavailable when another workspace has measured progress", () => {
    expect(
      idleSyncMessage({
        ...idleOptions,
        policies: [
          enabledPolicy({ uploadProgress }),
          enabledPolicy({ destinationId: "synthetic-team", storageKnown: false }),
        ],
      }),
    ).toBe("Status unavailable")
  })

  it("does not infer activity from automatic recovery in an idle summary", () => {
    expect(
      idleSyncMessage({
        ...idleOptions,
        policies: [enabledPolicy({ status: "fenced", automaticRecoveryCount: 1 })],
      }),
    ).toBe("Folder sync enabled")
  })

  it("marks retained scan measurements even if recovery work exists", () => {
    expect(
      idleSyncMessage({
        ...idleOptions,
        policies: [enabledPolicy({ uploadProgress, automaticRecoveryCount: 1 })],
      }),
    ).toBe("Last full scan • 1 of 5 sessions uploaded • 12 MB of 85 MB transferred")
  })

  it("keeps a discovery gap visible with the observed counts", () => {
    expect(
      idleSyncMessage({
        ...idleOptions,
        policies: [
          enabledPolicy({
            uploadProgress: { uploaded: 1, total: null, found: 5, scanIncomplete: true },
          }),
        ],
      }),
    ).toBe("1 session uploaded • 5 sessions found • Folder scan incomplete")
  })

  it("does not replace an error with progress", () => {
    expect(
      idleSyncMessage({
        ...idleOptions,
        hasError: true,
        policies: [enabledPolicy({ uploadProgress })],
      }),
    ).toBe("Folder sync needs attention")
  })

  it("keeps an unavailable workspace ahead of progress", () => {
    expect(
      idleSyncMessage({
        ...idleOptions,
        workspaceUnavailable: true,
        policies: [enabledPolicy({ uploadProgress })],
      }),
    ).toBe("Workspace unavailable")
  })

  it.each([
    { status: "offline", expected: "Cloud offline" },
    { status: "route_denied", expected: "Workspace denied uploads" },
    { status: "reauth_required", expected: "Sign-in or workspace access required" },
    { status: "upload_unavailable", expected: "Uploads unavailable" },
    { status: "storage_unavailable", expected: "Status unavailable" },
    { status: "fenced", expected: "Folder sync needs attention" },
    { status: "interrupted", expected: "Folder sync needs attention" },
  ] satisfies { status: CloudOngoingStatus["status"]; expected: string }[])(
    "keeps $status visible instead of progress",
    ({ status, expected }) => {
      expect(
        idleSyncMessage({
          ...idleOptions,
          policies: [enabledPolicy({ status, uploadProgress })],
        }),
      ).toBe(expected)
    },
  )

  it.each([{ folderConfirmationRequired: true }, { sourceConfirmationRequired: true }])(
    "keeps saved-folder review ahead of progress",
    (patch) => {
      expect(
        idleSyncMessage({
          ...idleOptions,
          policies: [enabledPolicy({ ...patch, uploadProgress })],
        }),
      ).toBe("Review saved folders")
    },
  )

  it("keeps the initial status until policies arrive", () => {
    expect(idleSyncMessage({ ...idleOptions, policies: [] })).toBe("Checking folder sync…")
  })

  it("does not show progress before folder consent", () => {
    expect(
      idleSyncMessage({
        ...idleOptions,
        policies: [enabledPolicy({ consentVersion: null, uploadProgress })],
      }),
    ).toBe("Folder sync not started")
  })

  it("does not show progress without approved folders", () => {
    expect(
      idleSyncMessage({
        ...idleOptions,
        policies: [enabledPolicy({ projects: [], uploadProgress })],
      }),
    ).toBe("Folder sync not started")
  })

  it.each([
    { deviceEnabled: false, policy: enabledPolicy({ uploadProgress }) },
    { deviceEnabled: true, policy: enabledPolicy({ paused: true, uploadProgress }) },
  ])("keeps the device or folder pause ahead of progress", ({ deviceEnabled, policy }) => {
    expect(idleSyncMessage({ ...idleOptions, deviceEnabled, policies: [policy] })).toBe(
      "Folder sync paused",
    )
  })

  it("waits for device status without showing stale progress", () => {
    expect(
      idleSyncMessage({
        ...idleOptions,
        deviceEnabled: undefined,
        policies: [enabledPolicy({ uploadProgress })],
      }),
    ).toBe("Checking folder sync…")
  })

  it("keeps unavailable storage ahead of cached progress", () => {
    expect(
      idleSyncMessage({
        ...idleOptions,
        policies: [enabledPolicy({ storageKnown: false, uploadProgress })],
      }),
    ).toBe("Status unavailable")
  })
})

describe("automatic recovery presentation", () => {
  it("requires an observed recovery count", () => {
    expect(onlyAutomaticRecovery(enabledPolicy())).toBe(false)
    expect(onlyAutomaticRecovery(enabledPolicy({ automaticRecoveryCount: 0 }))).toBe(false)
    expect(onlyAutomaticRecovery(enabledPolicy({ automaticRecoveryCount: 1 }))).toBe(true)
  })

  it.each([
    { status: "fenced", fencedCount: 1 },
    { status: "interrupted", interruptedCount: 1 },
  ] satisfies Partial<CloudOngoingStatus>[])(
    "does not hide $status errors while another upload can recover",
    (patch) => {
      const policy = enabledPolicy({ ...patch, automaticRecoveryCount: 1, uploadProgress })
      expect(onlyAutomaticRecovery(policy)).toBe(false)
      expect(savedUploadDisplayState(policy)).toBe(policy.status)
      expect(idleSyncMessage({ ...idleOptions, policies: [policy] })).toBe(
        "Folder sync needs attention",
      )
    },
  )

  it.each(["fenced", "interrupted"] as const)(
    "shows syncing for %s only when every affected upload has automatic recovery",
    (status) => {
      expect(
        savedUploadDisplayState(enabledPolicy({ status, automaticRecoveryCount: 1 })),
      ).toBe("syncing")
    },
  )
})
