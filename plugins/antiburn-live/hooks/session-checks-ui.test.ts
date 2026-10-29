import { expect, test } from "claude-code/testing";
import {
  applySessionChecks,
  disconnectedSessionChecks,
  parseSessionChecks,
  sessionCheckView,
  sessionConnectionText,
  type SessionChecksDelivery,
} from "./session-checks-ui";
import type { LiveViewState } from "../types";

const initial: LiveViewState = { checks: [], resolvedLine: [], entered: [] };

function delivery(overrides: Partial<SessionChecksDelivery> = {}): SessionChecksDelivery {
  return {
    protocol_version: 1,
    session_id: "session-1",
    identity: { environment_key: "native", agent: "claude-code", session_id: "session-1" },
    session_scope: "main",
    freshness_evaluated: false,
    state: "ready",
    snapshot: [{
      id: "dynamic-check-42",
      check_id: "predefined-yaml-check",
      name: "Review the risky change",
      message: "Check the affected files before continuing.",
      status: "failure",
      assessed_at_epoch: 1_800_000_000,
      checking: false,
      eligibility: "ready",
    }],
    updates: [],
    receipt: "a".repeat(64),
    ...overrides,
  };
}

test("parser accepts a main Claude snapshot with dynamic IDs and rejects mismatches", () => {
  const value = delivery();
  expect(parseSessionChecks(JSON.stringify(value), "session-1")?.snapshot[0]?.id).toBe("dynamic-check-42");
  expect(parseSessionChecks(JSON.stringify(value), "other-session")).toBeUndefined();
  expect(parseSessionChecks(JSON.stringify({ ...value, identity: { ...value.identity, agent: "codex" } }), "session-1")).toBeUndefined();
  expect(parseSessionChecks(JSON.stringify({ ...value, session_scope: "unsupported" }), "session-1")).toBeUndefined();
});

test("a waiting discovery response does not remove last-known findings", () => {
  const active = applySessionChecks(initial, delivery());
  const waiting = delivery({
    state: "waiting",
    session_scope: "unsupported",
    identity: undefined,
    snapshot: [],
    receipt: null,
  });
  const next = applySessionChecks(active, waiting);
  expect(next.checks.map((check) => check.id)).toEqual(["dynamic-check-42"]);
  expect(next.connection).toBe("waiting");
  expect(sessionConnectionText(next.connection)).toMatch(/Waiting for Antiburn/);
});

test("disconnection keeps last-known checks and reports their status", () => {
  const active = applySessionChecks(initial, delivery());
  const next = disconnectedSessionChecks(active);
  expect(next.checks).toEqual(active.checks);
  expect(next.connection).toBe("disconnected");
  expect(sessionConnectionText(next.connection)).toMatch(/last known checks/);
});

test("inactive delivery state has neutral copy", () => {
  expect(sessionConnectionText("inactive")).toBe("Antiburn checks are inactive for this session");
});

test("an explicit clear resolves a warning and invalidation removes it", () => {
  const active = applySessionChecks(initial, delivery());
  const cleared = applySessionChecks(active, delivery({
    snapshot: [{ ...delivery().snapshot[0]!, status: "success", message: null }],
    updates: [{ id: "dynamic-check-42", kind: "cleared", name: "Review the risky change", message: null }],
  }));
  expect(cleared.checks[0]?.stage).toBe("resolved");
  const invalidated = applySessionChecks(cleared, delivery({
    snapshot: [],
    updates: [{ id: "dynamic-check-42", kind: "invalidated", name: "dynamic-check-42", message: null }],
  }));
  expect(invalidated.checks).toEqual([]);
});

test("waiting applies authoritative invalidation but not absence-based clearing", () => {
  const active = applySessionChecks(initial, delivery());
  const invalidated = applySessionChecks(active, delivery({
    state: "waiting",
    snapshot: [],
    updates: [{ id: "dynamic-check-42", kind: "invalidated", name: "dynamic-check-42", message: null }],
  }));
  expect(invalidated.checks).toEqual([]);

  const retained = applySessionChecks(active, delivery({
    state: "waiting",
    snapshot: [],
    updates: [],
  }));
  expect(retained.checks.map((check) => check.id)).toEqual(["dynamic-check-42"]);
});

test("an authoritative success snapshot resolves a warning without implying repair", () => {
  const active = applySessionChecks(initial, delivery());
  const next = applySessionChecks(active, delivery({
    snapshot: [{ ...delivery().snapshot[0]!, status: "success", message: null }],
  }));
  expect(next.checks[0]?.stage).toBe("resolved");
  expect(next.checks[0]?.metric).toBe("Assessment no longer reports this warning");
});

test("a persisted failure remains visible while its next assessment is checking", () => {
  const prior = sessionCheckView(delivery().snapshot[0]!);
  const current: LiveViewState = { ...initial, checks: [prior] };
  const checking = applySessionChecks(current, delivery({
    snapshot: [{ ...delivery().snapshot[0]!, status: null, checking: true, message: null }],
  }));
  expect(checking.checks[0]?.stage).toBe("active");
  expect(checking.checks[0]?.checking).toBe(true);
  expect(checking.checks[0]?.metric).toBe("Checking for an updated result");
});

test("an out-of-range timestamp does not break the pane model", () => {
  const check = sessionCheckView({
    ...delivery().snapshot[0]!,
    assessed_at_epoch: Number.MAX_SAFE_INTEGER,
  });
  expect(check.metric).toBe("Assessment time unavailable");
});
