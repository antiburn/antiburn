import type { LiveCheckView, LiveViewState } from "../types";
import { validateSessionChecksDelivery } from "../../../packages/antiburn-plugin-core/session-checks-validation.mjs";

export type SessionCheckRow = {
  id: string;
  check_id: string;
  name: string;
  message: string | null;
  status: "failure" | "success" | null;
  assessed_at_epoch: number | null;
  checking: boolean;
  eligibility: string;
};

export type SessionCheckUpdate = {
  id: string;
  kind: "finding" | "cleared" | "invalidated";
  name: string;
  message: string | null;
};

export type SessionChecksDelivery = {
  protocol_version: 1;
  session_id: string;
  identity?: {
    environment_key: string;
    agent: string;
    session_id: string;
  };
  session_scope: "main" | "unsupported";
  freshness_evaluated: boolean;
  state: string;
  snapshot: SessionCheckRow[];
  updates: SessionCheckUpdate[];
  receipt: string | null;
};

export type SessionCheckConnection = "connected" | "waiting" | "disconnected" | "unavailable" | "inactive";

const MAX_OUTPUT = 256 * 1024;

export function parseSessionChecks(stdout: string, sessionId: string): SessionChecksDelivery | undefined {
  if (stdout.length === 0 || stdout.length > MAX_OUTPUT || sessionId.length === 0 || sessionId.length > 512)
    return undefined;
  try {
    const value: unknown = JSON.parse(stdout);
    return validateSessionChecksDelivery(value, { sessionId, host: "claude-code" }) as
      | SessionChecksDelivery
      | undefined;
  } catch {
    return undefined;
  }
}

function assessedAt(epoch: number | null): string {
  if (epoch === null) return "Assessment time unavailable";
  const date = new Date(epoch * 1000);
  if (!Number.isFinite(date.getTime())) return "Assessment time unavailable";
  return `Assessed ${date.toISOString().replace("T", " ").slice(0, 16)} UTC`;
}

export function sessionCheckView(row: SessionCheckRow, previous?: LiveCheckView): LiveCheckView {
  if (row.checking && row.status === null && previous?.source === "session" && previous.stage === "active")
    return { ...previous, checking: true, metric: "Checking for an updated result" };
  const stage = row.status === "failure" ? "active" : row.status === "success" ? "resolved" : "watching";
  const message = row.message?.trim() || "Antiburn has no advice for this result.";
  return {
    id: row.id,
    label: row.name,
    area: row.check_id,
    sentence: message,
    stage,
    metric: row.checking
      ? "Checking"
      : previous?.stage === "active" && row.status === "success"
        ? "Assessment no longer reports this warning"
        : row.status === null
          ? "Waiting for an assessment"
          : assessedAt(row.assessed_at_epoch),
    progress: row.checking ? "Checking" : null,
    cause: null,
    actionKind: "advise",
    actionLabel: "Review",
    prompt: message,
    isHidden: previous?.isHidden ?? false,
    isStoppedAfterFix: false,
    source: "session",
    checking: row.checking,
    assessedAt: row.assessed_at_epoch,
  };
}

export function disconnectedSessionChecks(current: LiveViewState): LiveViewState {
  return {
    ...current,
    connection: current.checks.some((check) => check.source === "session") ? "disconnected" : "waiting",
  };
}

export function applySessionChecks(
  current: LiveViewState,
  delivery: SessionChecksDelivery,
): LiveViewState {
  const currentSession = current.checks.filter((check) => check.source === "session");
  const previousById = new Map(currentSession.map((check) => [check.id, check]));
  const next = new Map<string, LiveCheckView>();
  const authoritative = delivery.session_scope === "main" && delivery.identity !== undefined;
  const waiting = delivery.state === "waiting" || delivery.state === "missing-database";
  const unavailable = delivery.state === "incompatible" || delivery.state === "unavailable";
  const inactive = delivery.state === "unsupported" || delivery.state === "unconfigured" || delivery.state === "disabled";

  // Waiting can carry source-block updates, but it cannot clear a warning by absence.
  if (authoritative) {
    for (const update of delivery.updates) {
      if (update.kind === "invalidated") continue;
      if (update.kind === "cleared") {
        const row = delivery.snapshot.find((item) => item.id === update.id);
        if (row) next.set(row.id, sessionCheckView(row, previousById.get(row.id)));
      }
    }
  }

  if (!authoritative || waiting || unavailable || inactive) {
    for (const previous of currentSession) {
      const update = delivery.updates.find((item) => item.id === previous.id);
      if (authoritative && update?.kind === "invalidated") continue;
      if (authoritative && update?.kind === "cleared") continue;
      if (inactive) continue;
      next.set(previous.id, previous);
    }
    for (const update of authoritative ? delivery.updates : []) {
      if (update.kind === "cleared") {
        const row = delivery.snapshot.find((item) => item.id === update.id);
        if (row) next.set(row.id, sessionCheckView(row, previousById.get(row.id)));
      }
    }
    return {
      ...current,
      checks: [
        ...current.checks.filter((check) => check.source !== "session"),
        ...next.values(),
      ],
      connection: unavailable ? "unavailable" : inactive ? "inactive" : "waiting",
    };
  }

  for (const row of delivery.snapshot) {
    const previous = previousById.get(row.id);
    next.set(row.id, sessionCheckView(row, previous));
  }
  for (const update of delivery.updates) {
    if (update.kind === "invalidated") {
      next.delete(update.id);
    }
  }
  const checks = [
    ...current.checks.filter((check) => check.source !== "session"),
    ...next.values(),
  ];
  const findings = delivery.updates.filter((item) => item.kind === "finding").length;
  return {
    ...current,
    checks,
    resolvedLine: [],
    entered: [],
    connection: "connected",
    freshnessEvaluated: delivery.freshness_evaluated,
    newSessionCount: findings,
  };
}

export function sessionConnectionText(connection: SessionCheckConnection | undefined, freshnessEvaluated?: boolean): string | undefined {
  if (connection === "disconnected") return "Antiburn disconnected · showing last known checks";
  if (connection === "unavailable") return "Antiburn could not read the latest checks · showing last known checks";
  if (connection === "inactive") return "Antiburn checks are inactive for this session";
  if (connection === "waiting") return "Waiting for Antiburn to discover this session";
  if (connection === "connected" && freshnessEvaluated === false) return "Connected · freshness not evaluated";
  return undefined;
}
