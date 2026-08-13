// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { confirm, save } from '@tauri-apps/plugin-dialog';
import { useCallback, useEffect, useState } from 'react';

import { SessionAnalyticsPresentation } from '../../components/session/SessionAnalyticsPresentation';
import { renderAgentIcon } from '../../lib/agentIcon';
import {
  deleteSessionData,
  exportSession,
  getSessionAnalytics,
  getSubagentAnalytics,
  revealSource,
  type SessionAnalyticsPayload,
} from '../../lib/ipc';
import { agentSupportsAnalytics } from '../../lib/presentation/agents';
import { costBreakdownRows, costFigureLabel } from '../../lib/presentation/sessionAnalytics';
import {
  topLevelCostSubject,
  type LocalSessionCost,
} from '../../lib/presentation/sessionCosts';
import type {
  LocalOrchestrationStatus,
  LocalSessionRelation,
  LocalSessionRelations,
} from '../../lib/types/session';

/**
 * One session's analytics, loaded and wired to the actions a reader can take.
 *
 * The presentation component owns every pixel; this owns the data it renders,
 * the navigation between sessions, and the three destructive-ish actions —
 * export, delete, and reveal — each of which is spelled out below because each
 * one is easy to get subtly wrong.
 */

/** Which session the pane is showing. */
export interface SessionSubject {
  agent: string;
  sessionId: string;
  wslDistro?: string | null | undefined;
  title?: string | undefined;
  isActive?: boolean | undefined;
  /** Present when the subject is a sub-agent rather than a session a reader drove. */
  subagent?: {
    parentSessionId: string;
    subagentId: string;
    parentTitle?: string;
  };
}

export interface SessionPaneProps {
  subject: SessionSubject;
  onBack: () => void;
  /** Newer adjacent session; omitted when there is none. */
  onPrev?: (() => void) | undefined;
  /** Older adjacent session; omitted when there is none. */
  onNext?: (() => void) | undefined;
  /** Navigate to another session (a fork, a sub-agent, an orchestrator). */
  onOpenSession: (subject: SessionSubject) => void;
  /** The session's local records were deleted, so it can no longer be shown. */
  onDeleted: () => void;
}

/**
 * A file name for an export that identifies the session without leaking its
 * title into the filesystem. The title can be anything; a slug plus a short id
 * is enough to tell two exports apart.
 */
function exportFileName(subject: SessionSubject): string {
  return `antiburn-${subject.agent}-${subject.sessionId.slice(0, 8)}.json`;
}

/**
 * The cost result the breakdown card describes.
 *
 * Token counts come from the metrics rather than the cost estimate, because the
 * estimate is dollars — the two are different views of the same subject and the
 * card shows both.
 */
function toLocalCost(
  subject: SessionSubject,
  payload: SessionAnalyticsPayload,
): LocalSessionCost | null {
  if (!payload.cost) return null;
  const metrics = payload.summary?.sessions[0];
  const inputTokens = metrics?.billableInputTokens ?? 0;
  const outputTokens = metrics?.billableOutputTokens ?? 0;
  const cacheReadTokens = metrics?.billableCacheReadTokens ?? 0;
  const cacheCreationTokens = metrics?.billableCacheCreationTokens ?? 0;
  return {
    subject: topLevelCostSubject(subject.agent, subject.sessionId, subject.wslDistro),
    inputTokens,
    outputTokens,
    cacheReadTokens,
    cacheCreationTokens,
    totalTokens: inputTokens + outputTokens + cacheReadTokens + cacheCreationTokens,
    inputCostUsd: payload.cost.inputUsd,
    outputCostUsd: payload.cost.outputUsd,
    cacheReadCostUsd: payload.cost.cacheReadUsd,
    cacheWriteCostUsd: payload.cost.cacheWriteUsd,
    totalCostUsd: payload.cost.totalUsd,
    model: metrics?.model ?? null,
    isActive: payload.isActive,
  };
}

/** Load one subject's analytics. Sub-agents come from their own command. */
async function loadAnalytics(subject: SessionSubject): Promise<SessionAnalyticsPayload | null> {
  if (subject.subagent) {
    return getSubagentAnalytics(
      subject.agent,
      subject.subagent.parentSessionId,
      subject.subagent.subagentId,
      subject.wslDistro,
    );
  }
  return getSessionAnalytics(subject.agent, subject.sessionId, subject.wslDistro);
}

export function SessionPane({
  subject,
  onBack,
  onPrev,
  onNext,
  onOpenSession,
  onDeleted,
}: SessionPaneProps) {
  /**
   * The load result, tagged with the session it belongs to.
   *
   * One piece of state rather than three, and it carries its own key: "still
   * loading" is then *derived* (the settled result is for a different session
   * than the one being shown) instead of being a flag an effect has to flip on
   * the way in. That keeps the effect body free of state updates, so opening a
   * session cannot cascade renders.
   */
  const [settled, setSettled] = useState<{
    key: string;
    payload: SessionAnalyticsPayload | null;
    error: boolean;
  } | null>(null);

  const key = `${subject.agent}|${subject.sessionId}|${subject.subagent?.subagentId ?? ''}`;

  useEffect(() => {
    let active = true;
    loadAnalytics(subject)
      .then((result) => {
        if (active) setSettled({ key, payload: result, error: false });
      })
      .catch(() => {
        if (active) setSettled({ key, payload: null, error: true });
      });
    return () => {
      active = false;
    };
    // `subject` is rebuilt on every render by the host, so the identity key is
    // what actually changes when a different session is opened.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key]);

  const current = settled?.key === key ? settled : null;
  const payload = current?.payload ?? null;
  const loading = current == null;
  const error = current?.error ?? false;

  /**
   * Export: confirm, then choose a destination, then write.
   *
   * The confirmation comes *first*, before the save dialog, so the reader
   * decides whether to export at all before being asked where — the reverse
   * order presents the warning after they have already committed to a folder
   * and a file name.
   */
  const handleExport = useCallback(async () => {
    const proceed = await confirm(
      'The export contains this session’s analysis, the paths it ran in, and short excerpts — its title and any skill descriptions. No message bodies, tool arguments, or file contents. It still describes real work, so save it somewhere you would keep a private note.',
      { title: 'Export session analysis?', kind: 'warning', okLabel: 'Choose destination…' },
    );
    if (!proceed) return;

    const destination = await save({
      defaultPath: exportFileName(subject),
      filters: [{ name: 'JSON', extensions: ['json'] }],
    });
    if (!destination) return;

    await exportSession(subject.agent, subject.sessionId, destination, subject.wslDistro);
  }, [subject]);

  /**
   * Delete: antiburn's own records only.
   *
   * The copy says so explicitly, because "delete session" in an app that reads
   * someone else's files could reasonably be read as deleting the conversation.
   * It does not: the agent's transcript is the agent's.
   */
  const handleDelete = useCallback(async () => {
    const proceed = await confirm(
      'This removes antiburn’s stored analysis for the session. The agent’s own transcript file is not touched, and a later scan will find the session again.',
      { title: 'Remove this session from antiburn?', kind: 'warning', okLabel: 'Remove' },
    );
    if (!proceed) return;
    await deleteSessionData(subject.agent, subject.sessionId, subject.wslDistro);
    onDeleted();
  }, [subject, onDeleted]);

  const sourcePath = payload?.sourcePath ?? null;
  const handleReveal = useCallback(() => {
    if (!sourcePath) return;
    void revealSource(sourcePath);
  }, [sourcePath]);

  const cost = payload ? toLocalCost(subject, payload) : null;
  const costBadge = payload?.cost
    ? {
        totalUsd: payload.cost.totalUsd,
        figureLabel: costFigureLabel(payload.isActive),
        models: payload.models,
        breakdownRows: costBreakdownRows(payload.cost),
      }
    : null;

  const orchestration: LocalOrchestrationStatus | null = payload?.orchestration ?? null;
  const relations: LocalSessionRelations | null = payload?.relations ?? null;
  // The stored title is the authority once it arrives; the one the list handed
  // over is what keeps the header from being blank in the meantime.
  const title = payload?.title ?? subject.title ?? undefined;

  const openRelated = useCallback(
    (target: LocalSessionRelation, title?: string) => {
      onOpenSession({
        agent: target.identity.agent,
        sessionId: target.identity.sessionId,
        wslDistro: target.identity.wslDistro ?? null,
        ...(title ? { title } : {}),
      });
    },
    [onOpenSession],
  );

  const openSubagent = useCallback(
    (parentAgent: string, parentSessionId: string, subagentId: string, label: string) => {
      onOpenSession({
        agent: parentAgent,
        sessionId: subagentId,
        wslDistro: subject.wslDistro ?? null,
        title: label,
        subagent: {
          parentSessionId,
          subagentId,
          ...(subject.title ? { parentTitle: subject.title } : {}),
        },
      });
    },
    [onOpenSession, subject.wslDistro, subject.title],
  );

  return (
    <SessionAnalyticsPresentation
      summary={payload?.summary ?? null}
      loading={loading}
      error={error}
      session={{
        agent: subject.agent,
        sessionId: subject.sessionId,
        ...(title ? { title } : {}),
        wslDistro: subject.wslDistro ?? null,
        isActive: payload?.isActive ?? subject.isActive ?? false,
        ...(subject.subagent ? { subagent: subject.subagent } : {}),
      }}
      supportsAnalytics={payload?.supportsAnalytics ?? agentSupportsAnalytics(subject.agent)}
      cost={cost}
      costBadge={costBadge}
      orchestration={orchestration}
      skills={payload?.skills ?? []}
      relations={relations}
      onBack={onBack}
      {...(onPrev ? { onPrev } : {})}
      {...(onNext ? { onNext } : {})}
      onOpenSubagent={openSubagent}
      {...(subject.subagent
        ? {
            onOpenOrchestrator: () =>
              onOpenSession({
                agent: subject.agent,
                sessionId: subject.subagent?.parentSessionId ?? subject.sessionId,
                wslDistro: subject.wslDistro ?? null,
                ...(subject.subagent?.parentTitle
                  ? { title: subject.subagent.parentTitle }
                  : {}),
              }),
          }
        : {})}
      onOpenRelatedSession={openRelated}
      onExportSession={() => void handleExport()}
      onDeleteSession={() => void handleDelete()}
      {...(sourcePath ? { onRevealSource: handleReveal } : {})}
      renderAgentIcon={renderAgentIcon}
    />
  );
}
