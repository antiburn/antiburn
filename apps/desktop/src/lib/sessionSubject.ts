import { getSessionAnalysis, getSubagentAnalysis, type SessionAnalysisPayload } from "./ipc"
import { localSessionKey } from "./presentation/localIdentity"

/** One top-level session or sub-agent that can supply a shared detail view. */
export interface SessionSubject {
  agent: string
  sessionId: string
  repo?: string | undefined
  timestamp?: string | undefined
  wslDistro?: string | null | undefined
  title?: string | undefined
  subagent?: {
    parentSessionId: string
    subagentId: string
    parentTitle?: string
  }
}

/** Identity key for a subject's analysis load. Stable across navigation. */
export function sessionKey(subject: SessionSubject): string {
  return subject.subagent
    ? JSON.stringify([
        "subagent",
        localSessionKey(subject.agent, subject.subagent.parentSessionId, subject.wslDistro),
        subject.subagent.subagentId,
      ])
    : localSessionKey(subject.agent, subject.sessionId, subject.wslDistro)
}

/** Load one subject's analysis. Sub-agents use their dedicated command. */
export async function loadSessionAnalysis(
  subject: SessionSubject,
): Promise<SessionAnalysisPayload | null> {
  if (subject.subagent) {
    return getSubagentAnalysis(
      subject.agent,
      subject.subagent.parentSessionId,
      subject.subagent.subagentId,
      subject.wslDistro,
    )
  }
  return getSessionAnalysis(subject.agent, subject.sessionId, subject.wslDistro)
}
