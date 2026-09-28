import { invoke, isTauri } from "@tauri-apps/api/core"

import type { SessionSearchEntry } from "./sessionSearchIpc"

export type SessionEvidenceKind =
  "user" | "assistant" | "thinking" | "tool_input" | "tool_result" | "tool_error"

export type SessionEvidenceScope = "main" | "delegated"

export interface EvidenceReference {
  key: string
  environmentKey: string
  agent: string
  sessionId: string
  sourceGeneration: number
  publishedFence: number
  sourceKey: string
  threadId: string
  scope: SessionEvidenceScope
  turnRowId: number
  turnIndex: number
  partIndex: number
  /** Unicode scalar offsets in raw text or the decoded JSON field. */
  matchStart?: number | null
  matchEnd?: number | null
  jsonPath?: string | null
}

export interface SessionEvidenceCoverage {
  state: "complete" | "partial" | "progressive"
  inspectedBytes: number
  byteLimit: number
}

export interface SessionEvidenceHit {
  session: SessionSearchEntry
  reference: EvidenceReference
  excerpt: string
  kind: SessionEvidenceKind
  score: number
  retrievalRank?: number
  coverage: SessionEvidenceCoverage
  truncated: boolean
}

export interface SessionEvidenceContextItem {
  reference: EvidenceReference
  kind: SessionEvidenceKind
  text: string
  truncated: boolean
}

export interface SessionEvidenceContextResponse {
  available: boolean
  reason: "stale" | "missing" | "unavailable" | null
  session: SessionSearchEntry | null
  previous: SessionEvidenceContextItem | null
  match: SessionEvidenceContextItem | null
  next: SessionEvidenceContextItem | null
}

export async function fetchSessionEvidence(
  reference: EvidenceReference,
): Promise<SessionEvidenceContextResponse> {
  if (!reference.key) throw new Error("Invalid evidence reference")
  if (!isTauri()) {
    return {
      available: false,
      reason: "unavailable",
      session: null,
      previous: null,
      match: null,
      next: null,
    }
  }
  return invoke<SessionEvidenceContextResponse>("fetch_session_evidence", { reference })
}
