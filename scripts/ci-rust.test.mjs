import assert from "node:assert/strict";
import { readdirSync, readFileSync } from "node:fs";
import path from "node:path";
import test from "node:test";
import { parse } from "yaml";

import {
  parseArguments,
  parseWorkspaceList,
  planRustChecks,
} from "./ci-rust.mjs";
import { WORKSPACES } from "./ci-workspaces.mjs";

const ALL_IDS = WORKSPACES.map((workspace) => workspace.id);

function labels(plan) {
  return plan.steps.map((step) => `${step.workspace}: ${step.step}`);
}

test("runs workspaces in the registry order, whatever the input order", () => {
  const plan = planRustChecks({
    workspaces: [...ALL_IDS].reverse(),
    platform: "linux",
  });
  const order = [...new Set(plan.steps.map((step) => step.workspace))];
  assert.deepEqual(order, ALL_IDS);
  assert.equal(order[0], "checks");
  assert.equal(order[3], "shell");
  assert.deepEqual(plan.skipped, []);
});

test("runs fmt, then Clippy, then tests in each workspace", () => {
  const plan = planRustChecks({
    workspaces: "trace,engine",
    fmt: true,
    platform: "linux",
  });
  assert.deepEqual(labels(plan), [
    "engine: fmt",
    "engine: clippy",
    "engine: tests",
    "engine: doctests",
    "trace: fmt",
    "trace: clippy",
    "trace: tests",
  ]);
  assert.deepEqual(plan.steps[0].args, ["fmt", "--check"]);
});

test("omits fmt without the flag", () => {
  const plan = planRustChecks({ workspaces: "hud", platform: "linux" });
  assert.ok(plan.steps.every((step) => step.step !== "fmt"));
});

test("uses the per-workspace commands and directories", () => {
  const plan = planRustChecks({
    workspaces: "engine,remote,shell,nudge,tray-icon",
    platform: "linux",
  });
  const byLabel = Object.fromEntries(
    plan.steps.map((step) => [`${step.workspace}: ${step.step}`, step]),
  );
  assert.deepEqual(byLabel["engine: clippy"].args, [
    "clippy",
    "--all-targets",
    "--locked",
    "--",
    "-D",
    "warnings",
  ]);
  assert.deepEqual(byLabel["engine: tests"].args, [
    "nextest",
    "run",
    "--locked",
    "--no-tests=pass",
    "--lib",
    "--tests",
  ]);
  assert.equal(byLabel["engine: tests"].cwd, "crates/antiburn-local");
  assert.deepEqual(byLabel["remote: tests"].args, [
    "nextest",
    "run",
    "--locked",
    "--no-tests=pass",
  ]);
  assert.equal(byLabel["remote: tests"].cwd, "crates/antiburn-remote");
  assert.deepEqual(byLabel["shell: clippy (analytics)"].args, [
    "clippy",
    "--all-targets",
    "--locked",
    "--features",
    "analytics",
    "--",
    "-D",
    "warnings",
  ]);
  assert.deepEqual(byLabel["shell: tests (analytics)"].args, [
    "nextest",
    "run",
    "--locked",
    "--no-tests=pass",
    "--features",
    "analytics",
  ]);
  assert.equal(
    byLabel["shell: tests (analytics)"].cwd,
    "apps/desktop/src-tauri",
  );
  assert.deepEqual(byLabel["nudge: tests"].args, [
    "nextest",
    "run",
    "--locked",
    "--no-tests=pass",
  ]);
  assert.equal(
    byLabel["nudge: tests"].cwd,
    "apps/desktop/src-tauri/crates/nudge",
  );
  assert.deepEqual(byLabel["tray-icon: clippy"].args, [
    "clippy",
    "--all-targets",
    "--locked",
    "--",
    "-D",
    "warnings",
  ]);
  assert.deepEqual(byLabel["tray-icon: tests"].args, [
    "test",
    "--locked",
    "--lib",
    "--test",
    "macos_highlight_override",
  ]);
  assert.ok(
    plan.steps.every(
      (step) => step.step === "fmt" || step.args.includes("--locked"),
    ),
  );
});

test("runs doctests after the tests only where the library has doctests", () => {
  const plan = planRustChecks({ workspaces: ALL_IDS, platform: "linux" });
  const doctests = plan.steps.filter((step) => step.step === "doctests");
  assert.deepEqual(
    doctests.map((step) => step.workspace),
    ["engine", "sound", "tray-icon"],
  );
  for (const step of doctests) {
    assert.deepEqual(step.args, ["test", "--locked", "--doc"]);
    assert.deepEqual(step.env, {});
    assert.equal(
      step.cwd,
      WORKSPACES.find((workspace) => workspace.id === step.workspace).directory,
    );
    const own = plan.steps.filter(
      (other) => other.workspace === step.workspace,
    );
    assert.equal(own.at(-1), step, `${step.workspace}: doctests run last`);
  }
  assert.deepEqual(
    labels(planRustChecks({ workspaces: "sound", platform: "linux" })),
    ["sound: clippy", "sound: tests", "sound: doctests"],
  );
});

test("sets test env only where the contract needs it", () => {
  const plan = planRustChecks({
    workspaces: "shell,tray-icon,trace",
    platform: "linux",
  });
  const byLabel = Object.fromEntries(
    plan.steps.map((step) => [`${step.workspace}: ${step.step}`, step]),
  );
  assert.deepEqual(byLabel["shell: tests (analytics)"].env, {
    ANTIBURN_ANALYTICS_URL: "http://127.0.0.1:8787",
    ANTIBURN_ANALYTICS_OPERATOR: "Local development",
  });
  assert.deepEqual(byLabel["shell: clippy (analytics)"].env, {});
  assert.deepEqual(byLabel["tray-icon: tests"].env, {
    TRAY_ICON_RUN_NATIVE_TEST: "1",
  });
  assert.deepEqual(byLabel["tray-icon: clippy"].env, {});
  assert.deepEqual(byLabel["trace: tests"].env, {});
  assert.ok(
    plan.steps.every((step) => !("CARGO_TARGET_DIR" in step.env)),
    "commands inherit CARGO_TARGET_DIR from the job",
  );
});

test("adds a shell Clippy pass without analytics only on request", () => {
  const without = planRustChecks({ workspaces: "shell", platform: "linux" });
  assert.deepEqual(labels(without), [
    "shell: clippy (analytics)",
    "shell: tests (analytics)",
  ]);
  const withCheck = planRustChecks({
    workspaces: "shell",
    fmt: true,
    noAnalyticsCheck: true,
    platform: "linux",
  });
  assert.deepEqual(labels(withCheck), [
    "shell: fmt",
    "shell: clippy (analytics)",
    "shell: clippy (no analytics)",
    "shell: tests (analytics)",
  ]);
  assert.deepEqual(withCheck.steps[2].args, [
    "clippy",
    "--all-targets",
    "--locked",
    "--",
    "-D",
    "warnings",
  ]);
  const otherWorkspace = planRustChecks({
    workspaces: "hud",
    noAnalyticsCheck: true,
    platform: "linux",
  });
  assert.deepEqual(labels(otherWorkspace), ["hud: clippy", "hud: tests"]);
});

test("runs the remote helper only on Linux hosts", () => {
  for (const platform of ["darwin", "win32"]) {
    const plan = planRustChecks({ workspaces: "engine,remote", platform });
    assert.deepEqual(
      [...new Set(plan.steps.map((step) => step.workspace))],
      ["engine"],
    );
    assert.equal(plan.skipped.length, 1);
    assert.equal(plan.skipped[0].workspace, "remote");
  }
  const linux = planRustChecks({
    workspaces: "engine,remote",
    platform: "linux",
  });
  assert.deepEqual(
    [...new Set(linux.steps.map((step) => step.workspace))],
    ["engine", "remote"],
  );
});

test("spawns cargo.exe on Windows and cargo elsewhere", () => {
  const windows = planRustChecks({ workspaces: "hud", platform: "win32" });
  assert.ok(windows.steps.every((step) => step.program === "cargo.exe"));
  const macos = planRustChecks({ workspaces: "hud", platform: "darwin" });
  assert.ok(macos.steps.every((step) => step.program === "cargo"));
});

test("plans Clippy and tests for every registry workspace", () => {
  for (const workspace of WORKSPACES) {
    const plan = planRustChecks({
      workspaces: workspace.id,
      fmt: true,
      platform: "linux",
    });
    assert.deepEqual(plan.skipped, [], workspace.id);
    const steps = plan.steps.map((step) => step.step);
    assert.equal(steps[0], "fmt", workspace.id);
    assert.ok(
      steps.some((step) => step.startsWith("clippy")),
      `${workspace.id}: clippy`,
    );
    assert.ok(
      steps.some((step) => step.startsWith("tests")),
      `${workspace.id}: tests`,
    );
    assert.equal(steps.includes("doctests"), Boolean(workspace.doctests));
    for (const step of plan.steps) {
      assert.equal(step.workspace, workspace.id);
      assert.equal(step.cwd, workspace.directory);
      assert.ok(step.args.length > 0);
    }
  }
  const linuxOnly = WORKSPACES.filter((workspace) => workspace.linuxOnly);
  for (const workspace of linuxOnly) {
    const plan = planRustChecks({
      workspaces: workspace.id,
      platform: "darwin",
    });
    assert.deepEqual(plan.steps, []);
    assert.equal(plan.skipped[0].workspace, workspace.id);
  }
});

test("rejects unknown workspace ids", () => {
  assert.throws(
    () => planRustChecks({ workspaces: "engine,desktop", platform: "linux" }),
    /Unknown Rust workspace id: desktop/,
  );
  assert.throws(() => parseWorkspaceList("Shell"), /Unknown/);
});

test("plans nothing for an empty list", () => {
  for (const workspaces of ["", " , ", []]) {
    const plan = planRustChecks({ workspaces, fmt: true, platform: "linux" });
    assert.deepEqual(plan, { steps: [], skipped: [] });
  }
});

test("removes duplicate ids", () => {
  const plan = planRustChecks({ workspaces: "hud,hud", platform: "linux" });
  assert.deepEqual(labels(plan), ["hud: clippy", "hud: tests"]);
});

test("parses the command-line flags", () => {
  assert.deepEqual(
    parseArguments([
      "--workspaces",
      "shell,hud",
      "--fmt",
      "--no-analytics-check",
      "--dry-run",
    ]),
    {
      workspaces: "shell,hud",
      fmt: true,
      noAnalyticsCheck: true,
      dryRun: true,
    },
  );
  assert.deepEqual(parseArguments(["--workspaces", ""]), {
    workspaces: "",
    fmt: false,
    noAnalyticsCheck: false,
    dryRun: false,
  });
  assert.equal(parseArguments(["--workspaces=engine"]).workspaces, "engine");
  assert.throws(() => parseArguments([]), /--workspaces is required/);
  assert.throws(() => parseArguments(["--workspaces"]), /needs a value/);
  assert.throws(
    () => parseArguments(["--workspaces", "hud", "--release"]),
    /Unknown argument/,
  );
});

// rust-cache needs the workspace directories in the workflow file. The first
// workspace of a group maps to the shared CARGO_TARGET_DIR; the others add
// only their lockfiles to the cache key.
test("the rust-cache workspace lists in ci.yml match the registry", () => {
  const ci = parse(
    readFileSync(
      new URL("../.github/workflows/ci.yml", import.meta.url),
      "utf8",
    ),
  );
  for (const [job, group] of [
    ["engine", "engine"],
    ["desktop-backend", "desktop"],
  ]) {
    const env = ci.jobs[job].env;
    assert.equal(
      env.CARGO_TARGET_DIR,
      "${{ github.workspace }}/target/ci",
      job,
    );
    const cache = ci.jobs[job].steps.find((step) =>
      step.uses?.startsWith("Swatinem/rust-cache@"),
    );
    const entries = cache.with.workspaces
      .trim()
      .split("\n")
      .map((line) => line.split("->").map((part) => part.trim()));
    const expected = WORKSPACES.filter(
      (workspace) => workspace.group === group,
    );
    assert.deepEqual(
      entries.map(([directory]) => directory),
      expected.map((workspace) => workspace.directory),
      `${job}: the rust-cache workspaces differ from scripts/ci-workspaces.mjs`,
    );
    const [[first, target], ...rest] = entries;
    assert.equal(
      path.posix.normalize(path.posix.join(first, target)),
      "target/ci",
      `${job}: ${first} must map to the shared target directory`,
    );
    for (const entry of rest) assert.equal(entry.length, 1, entry.join(" -> "));
  }
});

// The repository cache limit is 10 GB. A cache for every desktop part and
// every platform used 16.5 GB, so GitHub evicted the release caches.
test("CI saves Rust caches only where pull requests restore them", () => {
  const ci = parse(
    readFileSync(
      new URL("../.github/workflows/ci.yml", import.meta.url),
      "utf8",
    ),
  );
  const cacheStep = (job) =>
    ci.jobs[job].steps.find((step) =>
      step.uses?.startsWith("Swatinem/rust-cache@"),
    ).with;
  const engine = cacheStep("engine");
  assert.match(engine["save-if"], /!endsWith\(matrix\.name, '-arm64'\)/);
  const desktop = cacheStep("desktop-backend");
  assert.equal(desktop["shared-key"], "ci-desktop-${{ matrix.platform.name }}");
  assert.match(desktop["save-if"], /matrix\.part\.name == 'shell'/);
  assert.match(
    desktop["save-if"],
    /!endsWith\(matrix\.platform\.name, '-arm64'\)/,
  );
  for (const cache of [engine, desktop]) {
    assert.match(cache["save-if"], /github\.ref == 'refs\/heads\/main'/);
  }
});

// A suite that no workflow runs can fail on main without notice. Only the
// `node --test` arguments of step commands count; a comment or another field
// that names a suite does not run it.
test("a workflow runs every script test suite", () => {
  const scripts = new URL("./", import.meta.url);
  const workflows = new URL("../.github/workflows/", import.meta.url);
  const executed = new Set();
  for (const name of readdirSync(workflows)) {
    if (!/\.ya?ml$/.test(name)) continue;
    const workflow = parse(readFileSync(new URL(name, workflows), "utf8"));
    for (const job of Object.values(workflow.jobs ?? {})) {
      for (const step of job.steps ?? []) {
        if (typeof step.run !== "string") continue;
        for (const line of step.run.split("\n")) {
          const command = line.replace(/#.*$/, "");
          const args = command.match(/\bnode --test\s+(.*)$/)?.[1];
          if (!args) continue;
          for (const arg of args.split(/\s+/)) {
            if (arg.startsWith("scripts/")) executed.add(arg);
          }
        }
      }
    }
  }
  const missing = readdirSync(scripts)
    .filter((name) => name.endsWith(".test.mjs"))
    .filter((name) => !executed.has(`scripts/${name}`));
  assert.deepEqual(missing, []);
});
