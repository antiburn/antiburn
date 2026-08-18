// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

/**
 * Local cost subjects and the shaping the cost views need.
 *
 * A session's price is not one number. An orchestrator session has a
 * transcript of its own, a set of sub-agent transcripts it launched, and the
 * two taken together; a single sub-agent has only itself. Each of those is a
 * separate *subject*, priced independently, and confusing them is how a parent
 * ends up wearing its children's cost (or vice versa). So every figure in the
 * UI names the exact subject it describes, and {@link costSubjectKey} makes
 * that name a stable cache identity that a child can never collide with its
 * parent or a sibling.
 *
 * There is exactly one source of cost here: the on-device estimate the local
 * pricing table produces for a transcript on this machine. There is no second
 * layer to reconcile against.
 */

import { environmentKey } from './localIdentity';

/** The transcript or aggregate one cost result describes. */
export type LocalCostSubject =
  | {
      scope: 'topLevel' | 'subagents' | 'inclusive';
      agent: string;
      parentSessionId: string;
      wslDistro?: string | null;
    }
  | {
      scope: 'subagent';
      agent: string;
      parentSessionId: string;
      subagentId: string;
      wslDistro?: string | null;
    };

/** Billable token counts for one model within a result. */
interface LocalModelTokens {
  inputTokens?: number;
  outputTokens?: number;
  cacheReadTokens?: number;
  cacheCreationTokens?: number;
}

/** One priced cost subject, as the local pricing table computed it. */
export interface LocalSessionCost {
  subject: LocalCostSubject;
  inputTokens: number;
  outputTokens: number;
  cacheReadTokens: number;
  cacheCreationTokens: number;
  totalTokens: number;
  inputCostUsd: number;
  outputCostUsd: number;
  cacheReadCostUsd: number;
  cacheWriteCostUsd: number;
  totalCostUsd: number;
  /** The single model priced, when the subject used exactly one. */
  model?: string | null;
  /** Billable tokens per normalized model key; authoritative when mixed. */
  modelBreakdown?: Record<string, LocalModelTokens>;
  /** Whether the underlying transcript was still being written when priced. */
  isActive: boolean;
}

/** Every cost subject computed for one session in a single pass. */
export interface LocalSessionCostScopes {
  topLevel?: LocalSessionCost | null;
  subagents?: LocalSessionCost | null;
  inclusive?: LocalSessionCost | null;
  children: LocalSessionCost[];
}

/** The named session's own transcript, excluding anything it launched. */
export function topLevelCostSubject(
  agent: string,
  parentSessionId: string,
  wslDistro?: string | null,
): LocalCostSubject {
  return { scope: 'topLevel', agent, parentSessionId, ...(wslDistro ? { wslDistro } : {}) };
}

/** Every sub-agent the named session launched, together. */
export function subagentsCostSubject(
  agent: string,
  parentSessionId: string,
  wslDistro?: string | null,
): LocalCostSubject {
  return { scope: 'subagents', agent, parentSessionId, ...(wslDistro ? { wslDistro } : {}) };
}

/** The named session plus every sub-agent it launched. */
export function inclusiveCostSubject(
  agent: string,
  parentSessionId: string,
  wslDistro?: string | null,
): LocalCostSubject {
  return { scope: 'inclusive', agent, parentSessionId, ...(wslDistro ? { wslDistro } : {}) };
}

/** One named sub-agent of the named session. */
export function subagentCostSubject(
  agent: string,
  parentSessionId: string,
  subagentId: string,
  wslDistro?: string | null,
): LocalCostSubject {
  return {
    scope: 'subagent',
    agent,
    parentSessionId,
    subagentId,
    ...(wslDistro ? { wslDistro } : {}),
  };
}

/**
 * Stable cache identity for a subject.
 *
 * JSON-encoded as an array rather than joined, so an id containing the
 * separator cannot forge another subject's key. The environment comes first
 * and is case-normalized by {@link environmentKey}, so a native session and
 * its WSL namesake never share a price.
 */
export function costSubjectKey(subject: LocalCostSubject): string {
  return JSON.stringify([
    environmentKey(subject.wslDistro),
    subject.agent,
    subject.parentSessionId,
    subject.scope,
    subject.scope === 'subagent' ? subject.subagentId : null,
  ]);
}

/** True when two subjects name the same transcript or aggregate. */
export function sameCostSubject(a: LocalCostSubject, b: LocalCostSubject): boolean {
  return costSubjectKey(a) === costSubjectKey(b);
}

/** Flatten a one-pass pricing response into independently addressable results. */
export function costScopeResults(scopes: LocalSessionCostScopes): LocalSessionCost[] {
  return [
    scopes.topLevel ?? null,
    scopes.subagents ?? null,
    scopes.inclusive ?? null,
    ...scopes.children,
  ].filter((result): result is LocalSessionCost => result != null);
}

/** Pick the result for an exact subject out of a set, or null when absent. */
export function costForSubject(
  subject: LocalCostSubject,
  results: readonly LocalSessionCost[],
): LocalSessionCost | null {
  return results.find((result) => sameCostSubject(result.subject, subject)) ?? null;
}

/** The four component figures a breakdown renders, plus the total. */
export function resultComponentCost(result: LocalSessionCost): {
  totalUsd: number;
  inputUsd: number;
  outputUsd: number;
  cacheReadUsd: number;
  cacheWriteUsd: number;
} {
  return {
    totalUsd: result.totalCostUsd,
    inputUsd: result.inputCostUsd,
    outputUsd: result.outputCostUsd,
    cacheReadUsd: result.cacheReadCostUsd,
    cacheWriteUsd: result.cacheWriteCostUsd,
  };
}

/**
 * Every model that contributed billable tokens to this exact result, sorted so
 * the tooltip's order is stable. An aggregate subject can span model switches
 * and sub-agents, so its breakdown is authoritative; the singular `model`
 * remains the fallback for a result that carries no breakdown.
 */
export function resultModels(result: LocalSessionCost): string[] {
  const models = Object.keys(result.modelBreakdown ?? {})
    .map((model) => model.trim())
    .filter((model) => model.length > 0)
    .sort((a, b) => a.localeCompare(b));
  if (models.length > 0) return models;

  const fallback = result.model?.trim();
  return fallback ? [fallback] : [];
}
