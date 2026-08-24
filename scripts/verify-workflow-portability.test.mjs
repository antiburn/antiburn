// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

const workflow = readFileSync(
  new URL("../.github/workflows/ci.yml", import.meta.url),
  "utf8",
);
const packageJson = JSON.parse(
  readFileSync(new URL("../package.json", import.meta.url), "utf8"),
);

function namedStep(name) {
  const marker = `      - name: ${name}`;
  const start = workflow.indexOf(marker);
  assert.notEqual(start, -1, `${name} step must exist`);
  const next = workflow.indexOf("\n      - name:", start + marker.length);
  return workflow.slice(start, next === -1 ? undefined : next);
}

function jobBlock(id) {
  const marker = `\n  ${id}:\n`;
  const start = workflow.indexOf(marker);
  assert.notEqual(start, -1, `${id} job must exist`);
  const next = workflow.slice(start + marker.length).search(/\n  \S+:\n/);
  const end = next === -1 ? undefined : start + marker.length + next;
  return workflow.slice(start + 1, end);
}

function stepsIn(job) {
  const marker = "\n    steps:\n";
  const start = job.indexOf(marker);
  assert.notEqual(start, -1, "the job must declare steps");
  const steps = job.slice(start + marker.length);
  assert.match(steps, /^ {6}- \S+?:/, "the first step must be a block mapping");
  return steps
    .split(/\n(?= {6}- )/)
    .map((step) => step.replace(/^ {6}- /, "        "));
}

function keysAt(block, indent) {
  const pattern = new RegExp(`^ {${indent}}(?!#)(\\S+?):`, "gm");
  return [...block.matchAll(pattern)].map((match) => match[1]).sort();
}

function count(block, pattern) {
  return [...block.matchAll(pattern)].length;
}

function runBody(step) {
  const marker = "        run: |\n";
  const start = step.indexOf(marker);
  assert.notEqual(start, -1, "the step must declare a run block");
  return step
    .slice(start + marker.length)
    .replace(/^ {10}/gm, "")
    .trimEnd();
}

function git(cwd, ...args) {
  return execFileSync("git", args, { cwd, encoding: "utf8" }).trim();
}

function commit(cwd, message, signedOff = true) {
  const args = ["commit", "--allow-empty", "-m", message];
  if (signedOff) {
    args.push("-m", "Signed-off-by: Test User <test@example.com>");
  }
  git(cwd, ...args);
}

test("the DCO gate skips unsigned merges and rejects unsigned commits", (t) => {
  const job = jobBlock("dco");
  const steps = stepsIn(job);
  const script = runBody(steps[1]);
  assert.match(
    script,
    /git rev-list --no-merges "\$BASE_SHA"\.\."\$HEAD_SHA"/,
  );

  const repo = mkdtempSync(join(tmpdir(), "antiburn-dco-"));
  t.after(() => rmSync(repo, { recursive: true, force: true }));
  git(repo, "init", "--initial-branch=main");
  git(repo, "config", "user.name", "Test User");
  git(repo, "config", "user.email", "test@example.com");

  commit(repo, "Base commit");
  const baseSha = git(repo, "rev-parse", "HEAD");
  git(repo, "branch", "feature");
  commit(repo, "Main commit");
  git(repo, "checkout", "feature");
  commit(repo, "Feature commit");
  git(repo, "checkout", "main");
  git(repo, "merge", "--no-ff", "feature", "-m", "Unsigned merge");
  const mergeSha = git(repo, "rev-parse", "HEAD");

  const mergeResult = spawnSync("bash", ["-c", script], {
    cwd: repo,
    encoding: "utf8",
    env: { ...process.env, BASE_SHA: baseSha, HEAD_SHA: mergeSha },
  });
  assert.equal(mergeResult.status, 0, mergeResult.stderr || mergeResult.stdout);
  assert.doesNotMatch(mergeResult.stdout, new RegExp(mergeSha));

  commit(repo, "Unsigned authored commit", false);
  const unsignedSha = git(repo, "rev-parse", "HEAD");
  const commitResult = spawnSync("bash", ["-c", script], {
    cwd: repo,
    encoding: "utf8",
    env: { ...process.env, BASE_SHA: mergeSha, HEAD_SHA: unsignedSha },
  });
  assert.equal(commitResult.status, 1);
  assert.equal(
    commitResult.stdout.trim(),
    `::error::commit ${unsignedSha} is missing a Signed-off-by trailer (git commit -s)`,
  );
});

test("the cross-platform release cache target is shell-neutral", () => {
  const step = namedStep(
    "Compile the release target without bundling or signing",
  );

  assert.match(step, /--target "\$\{\{ matrix\.target \}\}" --no-bundle/);
  assert.doesNotMatch(step, /\$TARGET/);
});

test("the workflow declares no defaults that rebind a shell", () => {
  assert.deepEqual(keysAt(workflow, 0), [
    "concurrency",
    "env",
    "jobs",
    "name",
    "on",
    "permissions",
  ]);
});

test("the aislop job runs the pinned gate on the pull request base", () => {
  const job = jobBlock("aislop");
  assert.deepEqual(keysAt(job, 4), ["if", "name", "runs-on", "steps"]);
  assert.match(job, /^ {4}name: slop gate$/m);
  assert.match(job, /^ {4}if: github\.event_name == 'pull_request'$/m);
  assert.match(job, /^ {4}runs-on: ubuntu-24\.04$/m);

  const steps = stepsIn(job);
  assert.equal(steps.length, 5);

  assert.deepEqual(keysAt(steps[0], 8), ["name", "uses", "with"]);
  assert.deepEqual(keysAt(steps[0], 10), ["fetch-depth"]);
  assert.match(steps[0], /^ {8}name: Checkout with history$/m);
  assert.match(
    steps[0],
    /^ {8}uses: actions\/checkout@11d5960a326750d5838078e36cf38b85af677262 # v4$/m,
  );
  assert.match(steps[0], /^ {10}fetch-depth: 0$/m);

  assert.deepEqual(keysAt(steps[1], 8), ["name", "uses", "with"]);
  assert.deepEqual(keysAt(steps[1], 10), ["node-version"]);
  assert.match(steps[1], /^ {8}name: Install Node$/m);
  assert.match(
    steps[1],
    /^ {8}uses: actions\/setup-node@49933ea5288caeca8642d1e84afbd3f7d6820020 # v4$/m,
  );
  assert.match(
    steps[1],
    /^ {10}node-version: \$\{\{ env\.NODE_VERSION \}\}$/m,
  );

  assert.deepEqual(keysAt(steps[2], 8), ["name", "run"]);
  assert.deepEqual(keysAt(steps[2], 10), []);
  assert.match(steps[2], /^ {8}name: Enable Corepack$/m);
  assert.match(steps[2], /^ {8}run: corepack enable$/m);

  assert.deepEqual(keysAt(steps[3], 8), ["name", "run"]);
  assert.deepEqual(keysAt(steps[3], 10), []);
  assert.match(steps[3], /^ {8}name: Install dependencies$/m);
  assert.match(steps[3], /^ {8}run: pnpm install --frozen-lockfile$/m);

  assert.deepEqual(keysAt(steps[4], 8), ["env", "name", "run", "shell"]);
  assert.deepEqual(keysAt(steps[4], 10), ["BASE_SHA"]);
  assert.match(steps[4], /^ {8}name: Judge the changed files$/m);
  assert.match(steps[4], /^ {8}shell: bash$/m);
  assert.match(
    steps[4],
    /^ {10}BASE_SHA: \$\{\{ github\.event\.pull_request\.base\.sha \}\}$/m,
  );
  assert.match(
    steps[4],
    /^ {8}run: pnpm run slop --base "\$BASE_SHA"$/m,
  );
});

test("the slop script keeps the diff-scoped aislop command", () => {
  assert.equal(
    packageJson.scripts.slop,
    "aislop ci --changes --base origin/main",
  );
  assert.equal(packageJson.scripts.preslop, undefined);
  assert.equal(packageJson.scripts.postslop, undefined);
});

test("ci-required fails closed on the aislop job", () => {
  const job = jobBlock("ci-required");
  assert.deepEqual(keysAt(job, 4), [
    "env",
    "if",
    "name",
    "needs",
    "runs-on",
    "steps",
  ]);
  assert.match(job, /^ {4}name: ci-required$/m);
  assert.match(job, /^ {4}if: always\(\)$/m);
  assert.match(job, /^ {4}runs-on: ubuntu-24\.04$/m);
  assert.equal(count(job, /^ {6}- aislop$/gm), 1);
  assert.equal(
    count(job, /^ {6}AISLOP_RESULT: \$\{\{ needs\.aislop\.result \}\}$/gm),
    1,
  );
  assert.equal(count(job, /^ {6}AISLOP_RESULT:/gm), 1);
  assert.equal(
    count(job, /^ {6}EVENT_NAME: \$\{\{ github\.event_name \}\}$/gm),
    1,
  );
  assert.equal(count(job, /^ {6}EVENT_NAME:/gm), 1);

  const steps = stepsIn(job);
  assert.equal(steps.length, 1);
  assert.deepEqual(keysAt(steps[0], 8), ["name", "run", "shell"]);
  assert.match(steps[0], /^ {8}name: Require every selected gate$/m);
  assert.match(steps[0], /^ {8}shell: bash$/m);

  const script = runBody(steps[0]);
  const expectedScript = `set -euo pipefail
failed=0
require_success() {
  local name="$1" result="$2"
  if [[ "$result" != "success" ]]; then
    echo "::error::\${name} finished with \${result}, expected success."
    failed=1
  fi
}
allow_skip() {
  local name="$1" result="$2"
  if [[ "$result" != "success" && "$result" != "skipped" ]]; then
    echo "::error::\${name} finished with \${result}."
    failed=1
  fi
}
require_if_selected() {
  local name="$1" selected="$2" result="$3"
  if [[ "$selected" == "true" ]]; then
    require_success "$name" "$result"
  else
    allow_skip "$name" "$result"
  fi
}

require_success classify "$CLASSIFY_RESULT"
require_success boundary "$BOUNDARY_RESULT"
require_if_selected engine "$ENGINE_SELECTED" "$ENGINE_RESULT"
require_if_selected desktop-frontend "$FRONTEND_SELECTED" "$FRONTEND_RESULT"
require_if_selected desktop-backend "$BACKEND_SELECTED" "$BACKEND_RESULT"
if [[ "$RELEASE_APP_SELECTED" == "true" || "$RELEASE_ENGINE_SELECTED" == "true" ]]; then
  require_success release-metadata "$RELEASE_METADATA_RESULT"
else
  allow_skip release-metadata "$RELEASE_METADATA_RESULT"
fi
if [[ "$EVENT_NAME" == "push" && "$RELEASE_APP_SELECTED" == "true" ]]; then
  require_success warm-release-cache "$WARM_RELEASE_RESULT"
else
  allow_skip warm-release-cache "$WARM_RELEASE_RESULT"
fi
if [[ "$ENGINE_SELECTED" == "true" || "$BACKEND_SELECTED" == "true" ]]; then
  require_success licenses "$LICENSES_RESULT"
else
  allow_skip licenses "$LICENSES_RESULT"
fi
if [[ "$EVENT_NAME" == "pull_request" ]]; then
  require_success aislop "$AISLOP_RESULT"
else
  allow_skip aislop "$AISLOP_RESULT"
fi
if [[ "$EVENT_NAME" == "pull_request" ]]; then
  require_success dco "$DCO_RESULT"
else
  allow_skip dco "$DCO_RESULT"
fi
exit "$failed"`;
  assert.equal(script, expectedScript);

  const aislopBranch = `if [[ "$EVENT_NAME" == "pull_request" ]]; then
  require_success aislop "$AISLOP_RESULT"
else
  allow_skip aislop "$AISLOP_RESULT"
fi`;
  assert.match(script, new RegExp(aislopBranch.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")));
  assert.ok(script.indexOf(aislopBranch) < script.indexOf('exit "$failed"'));
});

test("desktop backend checks use parallel jobs with one aggregate result", () => {
  const format = jobBlock("desktop-backend-format");
  const checks = jobBlock("desktop-backend-checks");
  const aggregate = jobBlock("desktop-backend");

  assert.match(format, /^ {4}runs-on: ubuntu-latest$/m);
  assert.equal(count(format, /run: cargo fmt --check/g), 3);
  assert.doesNotMatch(format, /runner\.os/);

  assert.equal(
    count(checks, /^ {10}- name: (linux|windows|macos)$/gm),
    3,
  );
  assert.match(
    checks,
    /^ {4}if: needs\.classify\.outputs\.desktop_backend == 'true'$/m,
  );
  assert.match(
    checks,
    /^ {12}command: clippy --all-targets --locked -- -D warnings$/m,
  );
  assert.match(checks, /^ {12}command: test --locked$/m);
  assert.equal(count(checks, /^ {10}- name: (shell|diagnostic|HUD)$/gm), 3);
  assert.match(
    checks,
    /^ {8}working-directory: \$\{\{ matrix\.workspace\.directory \}\}$/m,
  );
  assert.match(
    checks,
    /^ {8}run: cargo \$\{\{ matrix\.check\.command \}\}$/m,
  );
  assert.match(
    checks,
    /^ {10}save-if: \$\{\{ matrix\.check\.save_cache && github\.event_name == 'push' && github\.ref == 'refs\/heads\/main' \}\}$/m,
  );

  for (const dependency of [
    "classify",
    "desktop-backend-format",
    "desktop-backend-checks",
  ]) {
    assert.equal(count(aggregate, new RegExp(`^ {6}- ${dependency}$`, "gm")), 1);
  }
  assert.match(aggregate, /^ {4}if: always\(\)$/m);
  assert.match(
    aggregate,
    /^ {6}BACKEND_SELECTED: \$\{\{ needs\.classify\.outputs\.desktop_backend \}\}$/m,
  );
});

test("the standalone HUD crate has dedicated backend checks", () => {
  const checks = jobBlock("desktop-backend-checks");
  assert.match(
    checks,
    /^ {10}- name: HUD\n {12}directory: apps\/desktop\/src-tauri\/crates\/hud$/m,
  );
  assert.match(
    checks,
    /^ {12}command: clippy --all-targets --locked -- -D warnings$/m,
  );
  assert.match(checks, /^ {12}command: test --locked$/m);

  const format = namedStep("Check HUD crate formatting");
  assert.match(format, /working-directory: apps\/desktop\/src-tauri\/crates\/hud/);
  assert.ok(format.includes("run: cargo fmt --check"));
});
