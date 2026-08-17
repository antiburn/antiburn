// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import assert from "node:assert/strict";
import test from "node:test";

import {
  parsePositiveInteger,
  requestTimeoutMilliseconds,
  selectTrustedMainCi,
} from "./wait-for-main-ci.mjs";

const SHA = "0123456789abcdef0123456789abcdef01234567";

function run(overrides = {}) {
  return {
    id: 1,
    event: "push",
    head_branch: "main",
    head_sha: SHA,
    status: "completed",
    conclusion: "success",
    html_url: "https://github.example/run/1",
    run_attempt: 1,
    ...overrides,
  };
}

test("accepts only a successful main push for the exact SHA", () => {
  assert.equal(selectTrustedMainCi([run()], SHA).state, "success");
  assert.equal(
    selectTrustedMainCi([run({ event: "pull_request" })], SHA).state,
    "missing",
  );
  assert.equal(
    selectTrustedMainCi([run({ head_branch: "feature" })], SHA).state,
    "missing",
  );
  assert.equal(
    selectTrustedMainCi([run({ head_sha: "different" })], SHA).state,
    "missing",
  );
});

test("waits for an in-progress exact run", () => {
  const selected = selectTrustedMainCi(
    [run({ status: "in_progress", conclusion: null })],
    SHA,
  );
  assert.equal(selected.state, "waiting");
});

test("fails when the exact main run completed unsuccessfully", () => {
  const selected = selectTrustedMainCi([run({ conclusion: "failure" })], SHA);
  assert.equal(selected.state, "failed");
  assert.equal(selected.run.conclusion, "failure");
});

test("accepts a successful rerun even when an earlier attempt failed", () => {
  const selected = selectTrustedMainCi(
    [run({ conclusion: "failure" }), run({ id: 2, run_attempt: 2 })],
    SHA,
  );
  assert.equal(selected.state, "success");
  assert.equal(selected.run.id, 2);
});

test("requires positive integer wait settings", () => {
  assert.equal(parsePositiveInteger("15", "poll"), 15);
  for (const value of ["0", "-1", "1.5", "5seconds", ""]) {
    assert.throws(
      () => parsePositiveInteger(value, "poll"),
      /positive integer/,
    );
  }
  assert.throws(
    () => parsePositiveInteger("99999999999999999999", "poll"),
    /too large/,
  );
});

test("bounds each API request by both 30 seconds and the total deadline", () => {
  assert.equal(requestTimeoutMilliseconds(100_000, 20_000), 30_000);
  assert.equal(requestTimeoutMilliseconds(25_000, 20_000), 5_000);
  assert.equal(requestTimeoutMilliseconds(20_000, 20_000), 1);
});
