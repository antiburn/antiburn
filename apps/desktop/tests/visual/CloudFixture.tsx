import { createCloudDevPort, isCloudDevScenario } from "../../src/onboarding/cloudDevPort"
import { CloudOnboarding } from "../../src/onboarding/CloudOnboarding"
import {
  emptyCloudSnapshot,
  type CloudPort,
  type CloudSnapshot,
  type SyncResult,
} from "../../src/lib/cloud"

export function cloudFixture(name: string): CloudPort {
  const dev = name.startsWith("dev-") ? name.slice(4) : null
  if (isCloudDevScenario(dev)) return createCloudDevPort(dev)
  const authenticated = [
    "identity",
    "identity_error",
    "folders",
    "empty",
    "complete",
    "accepted",
    "partial",
    "failed",
    "ambiguous",
    "syncing",
    "unavailable",
    "cancelled",
  ].includes(name)
  let value: CloudSnapshot = {
    ...emptyCloudSnapshot,
    authEnabled: !["marketing", "returned", "marketing_failed"].includes(name),
    syncEnabled: [
      "accepted",
      "partial",
      "failed",
      "ambiguous",
      "syncing",
      "cancelled",
    ].includes(name),
    status: authenticated
      ? "signed_in"
      : name === "marketing" || name === "auth"
        ? "idle"
        : name === "marketing_failed"
          ? "browser_failed"
          : (name as CloudSnapshot["status"]),
    identity: authenticated ? { id: "synthetic", login: "example" } : null,
    draft: {
      screen: ["identity", "identity_error"].includes(name)
        ? "identity"
        : name === "complete"
          ? "complete"
          : authenticated
            ? "folders"
            : "handoff",
      folders:
        name === "empty"
          ? []
          : [
              { path: "/Example/Studio", selected: true, sessionCount: 128 },
              { path: "/Example/Website", selected: true, sessionCount: 64 },
            ],
    },
  }
  let saveFailed = false
  return {
    read: async () => value,
    watch: async () => () => {},
    begin: async () => {
      value = { ...value, status: value.authEnabled ? "waiting" : "idle" }
      return value
    },
    cancel: async () => {
      value = { ...value, status: "cancelled" }
      return value
    },
    save: async (draft) => {
      if (name === "identity_error" && !saveFailed) {
        saveFailed = true
        throw new Error("Synthetic save failure")
      }
      value = { ...value, draft }
      return value
    },
    signOut: async () => {
      value = { ...emptyCloudSnapshot, status: "signed_out" }
      return value
    },
    pick: async () => ["/Example/New folder"],
    agent: async () => (name === "complete" ? "pending" : "unavailable"),
    prepareSync: async () => {},
    start: async () =>
      name === "ambiguous"
        ? Promise.reject(new Error("Synthetic network ambiguity"))
        : name === "failed"
          ? { status: "failed" }
          : {
              status: name as "accepted" | "partial" | "syncing",
              operationId: "synthetic-operation",
              completed: name === "partial" ? 1 : 0,
              total: 2,
            },
    reconcile: async (): Promise<SyncResult> => ({
      status: "accepted",
      operationId: "synthetic-operation",
      completed: 0,
      total: 2,
    }),
    cancelSync: async () => ({ status: "cancelled" }),
  }
}
export function CloudFixture() {
  const name = new URLSearchParams(window.location.search).get("cloud") ?? "marketing"
  return (
    <CloudOnboarding
      port={cloudFixture(name)}
      analytics={false}
      holdIdentity={name === "identity"}
      onBack={() => {}}
      onComplete={() => {
        document.body.dataset.completed = "true"
      }}
    />
  )
}
