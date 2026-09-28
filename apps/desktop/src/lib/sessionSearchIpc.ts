import type { SessionSearchScope } from "./sessionSearchScope"
import { invoke, isTauri } from "@tauri-apps/api/core"

export interface SessionSearchEntry {
  environmentKey: string
  agent: string
  sessionId: string
  wslDistro: string | null
  title: string | null
  repository: string
  cwdLabel: string
  models: string[]
  timestamp: string
}
export interface SessionSearchResponse {
  results: SessionSearchEntry[]
  nextCursor: string | null
  hasMore: boolean
  indexing: boolean
}

export async function searchSessions(
  query: string,
  cursor: string | null = null,
  scope?: SessionSearchScope | null,
): Promise<SessionSearchResponse> {
  if (!isTauri()) return { results: [], nextCursor: null, hasMore: false, indexing: false }
  return invoke<SessionSearchResponse>("search_sessions", {
    query,
    cursor,
    ...(scope === undefined ? {} : { scope }),
  })
}
