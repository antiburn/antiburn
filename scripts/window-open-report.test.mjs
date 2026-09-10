import assert from "node:assert/strict";
import test from "node:test";

import { collectSample, summarize } from "./window-open-report.mjs";

const event = (kind, elapsed, extra = {}) =>
  JSON.stringify({
    fields: {
      event: "main_window_revealed",
      window: "main",
      open_kind: kind,
      elapsed_ms: elapsed,
      ...extra,
    },
  });

test("ignores unrelated diagnostics and rejects malformed benchmark events", () => {
  const samples = [];
  collectSample(
    samples,
    JSON.stringify({ fields: { event: "scan_finished" } }),
  );
  collectSample(samples, "");
  assert.deepEqual(samples, []);
  assert.throws(() => collectSample(samples, "not JSON"), /JSON/);
  assert.throws(
    () => collectSample(samples, event("warm_reopen", -1)),
    /duration/,
  );
  assert.throws(
    () => collectSample(samples, event("warm_reopen", "25")),
    /duration/,
  );
  assert.throws(() => collectSample(samples, event("unknown", 25)), /kind/);
  assert.throws(
    () =>
      collectSample(samples, event("warm_reopen", 25, { window: "settings" })),
    /window/,
  );
});

test("keeps first opens separate from warm samples and uses nearest-rank p95", () => {
  const samples = [];
  collectSample(samples, event("cold_launch", 600));
  collectSample(samples, event("first_open", 400));
  for (let index = 1; index <= 30; index += 1) {
    collectSample(samples, event("warm_reopen", index));
  }
  const report = summarize(samples);
  assert.equal(report.groups.cold_launch.count, 1);
  assert.equal(report.groups.first_open.p95_ms, 400);
  assert.equal(report.groups.warm_reopen.median_ms, 15.5);
  assert.equal(report.groups.warm_reopen.p95_ms, 29);
  assert.equal(report.warm_native_target.status, "met");
  assert.equal(report.metric, "request_to_native_reveal_ms");
  assert.equal(report.presented_frame_measured, false);
});

test("does not pass a target with too few samples or no events", () => {
  assert.equal(summarize([]).warm_native_target.status, "insufficient_samples");
  const samples = [];
  for (let index = 0; index < 29; index += 1)
    collectSample(samples, event("warm_reopen", 1));
  assert.equal(
    summarize(samples).warm_native_target.status,
    "insufficient_samples",
  );
  collectSample(samples, event("warm_reopen", 1));
  assert.equal(summarize(samples).warm_native_target.status, "met");
});

test("reports a slow warm distribution without mixing cold launch data", () => {
  const samples = [];
  for (let index = 0; index < 30; index += 1)
    collectSample(samples, event("warm_reopen", 101));
  assert.equal(summarize(samples).warm_native_target.status, "missed");
});
