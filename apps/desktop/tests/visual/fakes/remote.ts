import type { RemoteHost, RemoteSyncStatus } from "../../../src/lib/remoteHosts"
import { emitFixtureEvent } from "./event"

export const remoteFixtureIds = [
  "11111111-1111-4111-8111-111111111111",
  "22222222-2222-4222-8222-222222222222",
] as const

const hosts: RemoteHost[] = remoteFixtureIds.map((id, index) => ({
  id,
  sshAlias: index === 0 ? "studio-linux" : "build-server",
  displayName: index === 0 ? "Studio Linux" : "Build server",
  status: index === 0 ? "idle" : "error",
  lastSuccessfulSyncEpoch: Date.parse("2026-09-14T23:58:00Z") / 1000,
  automaticSyncEnabled: true,
  cachedSessionCount: 1,
  lastError:
    index === 0 ? null : { category: "sshUnavailable", message: "SSH connection timed out" },
}))
let sync: RemoteSyncStatus = { intervalSecs: 300, active: null, pendingHostIds: [] }

export function hasRemoteFixture(): boolean {
  return new URLSearchParams(window.location.search).get("remote") === "1"
}

export function remoteFixtureCommand(
  command: string,
  args: Record<string, unknown> | undefined,
): unknown {
  switch (command) {
    case "get_remote_hosts":
      return hasRemoteFixture() ? hosts.map((host) => ({ ...host })) : []
    case "get_remote_sync_status":
      return sync
    case "check_remote_host": {
      const result = {
        status: "ready" as const,
        supportedAgents: ["claude-code", "codex"],
        platform: "linux",
        architecture: "aarch64",
        message: null,
      }
      if (new URLSearchParams(window.location.search).get("remoteCheck") === "pending") {
        return new Promise((resolve) => {
          window.__ANTIBURN_VISUAL_FINISH_REMOTE_CHECK__ = () => {
            delete window.__ANTIBURN_VISUAL_FINISH_REMOTE_CHECK__
            resolve(result)
          }
        })
      }
      return result
    }
    case "set_remote_host_sync_enabled": {
      const host = hosts.find((candidate) => candidate.id === args?.id)
      if (!host) throw new Error("Missing fixture host")
      host.automaticSyncEnabled = Boolean(args?.enabled)
      emitFixtureEvent(
        "remote-hosts-changed",
        hosts.map((item) => ({ ...item })),
      )
      return undefined
    }
    case "update_remote_host": {
      const host = hosts.find((candidate) => candidate.id === args?.id)
      if (!host) throw new Error("Missing fixture host")
      host.sshAlias = String(args?.sshAlias)
      host.displayName = (args?.displayName as string | null) ?? null
      emitFixtureEvent(
        "remote-hosts-changed",
        hosts.map((item) => ({ ...item })),
      )
      return { ...host }
    }
    case "add_remote_host": {
      const host: RemoteHost = {
        id: "33333333-3333-4333-8333-333333333333",
        sshAlias: String(args?.sshAlias),
        displayName: (args?.displayName as string | null) ?? null,
        status: "idle",
        lastSuccessfulSyncEpoch: null,
        automaticSyncEnabled: true,
        cachedSessionCount: 0,
        lastError: null,
      }
      hosts.push(host)
      emitFixtureEvent(
        "remote-hosts-changed",
        hosts.map((item) => ({ ...item })),
      )
      return host
    }
    case "remove_remote_host": {
      const index = hosts.findIndex((host) => host.id === args?.id)
      if (index >= 0) hosts.splice(index, 1)
      emitFixtureEvent(
        "remote-hosts-changed",
        hosts.map((item) => ({ ...item })),
      )
      return undefined
    }
    case "set_remote_sync_interval":
      sync = {
        ...sync,
        intervalSecs: Number(args?.seconds) as RemoteSyncStatus["intervalSecs"],
      }
      return sync
    default:
      return undefined
  }
}
