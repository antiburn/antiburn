import type { SessionSearchScope } from "./sessionSearchScope"
import { invoke, isTauri } from "@tauri-apps/api/core"
import type { SessionEvidenceHit } from "./sessionEvidenceIpc"

export type DeepSearchStatus =
  "searching" | "stopped" | "finished" | "finished_unavailable" | "partial"

export interface DeepSearchResponse {
  scope?: SessionSearchScope | null
  scanId: number
  queryRevision: number
  available: boolean
  status: DeepSearchStatus
  continuationAvailable: boolean
  results: SessionEvidenceHit[]
  invalidatedSessions: Array<{ environmentKey: string; agent: string; sessionId: string }>
  totalMatchingSessions: number
  coverage: {
    eligibleSessions: number
    inspectedSessions: number
    inspectedParts: number
    inspectedBytes: number
    unavailableSessions: number
    changedSessions: number
    ingestionTruncatedParts: number
    scopeExhausted: boolean
  }
}

function assertIdentity(scanId: number, queryRevision: number): void {
  if (![scanId, queryRevision].every((value) => Number.isSafeInteger(value) && value >= 0))
    throw new Error("Invalid scan identity")
}

export async function startDeepSessionSearch(
  query: string,
  scanId: number,
  queryRevision: number,
  scope?: SessionSearchScope | null,
): Promise<DeepSearchResponse> {
  assertIdentity(scanId, queryRevision)
  if (!query.trim() || Array.from(query).length > 200) throw new Error("Invalid content query")
  if (!isTauri()) throw new Error("Retained content search requires the desktop app")
  return invoke("start_deep_session_search", {
    query,
    scanId,
    queryRevision,
    ...(scope === undefined ? {} : { scope }),
  })
}

export async function continueDeepSessionSearch(
  scanId: number,
  queryRevision: number,
): Promise<DeepSearchResponse> {
  assertIdentity(scanId, queryRevision)
  if (!isTauri()) throw new Error("Retained content search requires the desktop app")
  return invoke("continue_deep_session_search", { scanId, queryRevision })
}

export async function cancelDeepSessionSearch(
  scanId: number,
  queryRevision: number,
  discard: boolean,
): Promise<void> {
  assertIdentity(scanId, queryRevision)
  if (!isTauri()) return
  await invoke("cancel_deep_session_search", { scanId, queryRevision, discard })
}
