#!/usr/bin/env node
// Runs the Rust checks for the selected Cargo workspaces.
//
//   node scripts/ci-rust.mjs --workspaces <ids> [--fmt] [--no-analytics-check] [--dry-run]
//
// Each workspace keeps its own lockfile. CI sets one CARGO_TARGET_DIR per job,
// and every command inherits it, so the workspaces share one build directory.
import { spawnSync } from "node:child_process";
import { dirname, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

import { WORKSPACES } from "./ci-workspaces.mjs";

const REPO_ROOT = join(dirname(fileURLToPath(import.meta.url)), "..");

/** Clippy on every target, with warnings as errors. */
function clippy(features = []) {
  const featureArgs =
    features.length > 0 ? ["--features", features.join(",")] : [];
  return [
    "clippy",
    "--all-targets",
    "--locked",
    ...featureArgs,
    "--",
    "-D",
    "warnings",
  ];
}

const CLIPPY = clippy();
const NEXTEST = ["nextest", "run", "--locked", "--no-tests=pass"];

const DOCTESTS = ["test", "--locked", "--doc"];

const SHELL_TEST_ENV = {
  ANTIBURN_ANALYTICS_URL: "http://127.0.0.1:8787",
  ANTIBURN_ANALYTICS_OPERATOR: "Local development",
};

export function parseWorkspaceList(value) {
  const ids = (value ?? "")
    .split(",")
    .map((id) => id.trim())
    .filter(Boolean);
  const known = new Set(WORKSPACES.map((workspace) => workspace.id));
  const unknown = ids.filter((id) => !known.has(id));
  if (unknown.length > 0) {
    throw new Error(
      `Unknown Rust workspace id: ${unknown.join(", ")}. Known ids: ${[...known].join(", ")}`,
    );
  }
  return ids;
}

function workspaceCommands({ id, doctests }, { fmt, noAnalyticsCheck }) {
  const commands = [];
  if (fmt) commands.push({ step: "fmt", args: ["fmt", "--check"] });
  switch (id) {
    case "engine":
      commands.push({ step: "clippy", args: CLIPPY });
      commands.push({ step: "tests", args: [...NEXTEST, "--lib", "--tests"] });
      break;
    case "shell":
      commands.push({
        step: "clippy (analytics)",
        args: clippy(["analytics"]),
      });
      if (noAnalyticsCheck) {
        commands.push({ step: "clippy (no analytics)", args: CLIPPY });
      }
      commands.push({
        step: "tests (analytics)",
        args: [...NEXTEST, "--features", "analytics"],
        env: SHELL_TEST_ENV,
      });
      break;
    case "tray-icon":
      commands.push({ step: "clippy", args: CLIPPY });
      // The native test target uses `harness = false` and runs only with this
      // opt-in variable, so it stays on `cargo test`.
      commands.push({
        step: "tests",
        args: [
          "test",
          "--locked",
          "--lib",
          "--test",
          "macos_highlight_override",
        ],
        env: { TRAY_ICON_RUN_NATIVE_TEST: "1" },
      });
      break;
    default:
      commands.push({ step: "clippy", args: CLIPPY });
      commands.push({ step: "tests", args: NEXTEST });
  }
  if (doctests) commands.push({ step: "doctests", args: DOCTESTS });
  return commands;
}

// Returns the ordered check plan. The function has no side effects.
export function planRustChecks({
  workspaces,
  fmt = false,
  noAnalyticsCheck = false,
  platform = process.platform,
}) {
  const ids = Array.isArray(workspaces)
    ? parseWorkspaceList(workspaces.join(","))
    : parseWorkspaceList(workspaces);
  const selected = new Set(ids);
  const program = platform === "win32" ? "cargo.exe" : "cargo";
  const steps = [];
  const skipped = [];
  for (const workspace of WORKSPACES) {
    if (!selected.has(workspace.id)) continue;
    if (workspace.linuxOnly && platform !== "linux") {
      skipped.push({
        workspace: workspace.id,
        reason: "the workspace targets Linux hosts only",
      });
      continue;
    }
    for (const command of workspaceCommands(workspace, {
      fmt,
      noAnalyticsCheck,
    })) {
      steps.push({
        workspace: workspace.id,
        step: command.step,
        cwd: workspace.directory,
        program,
        args: [...command.args],
        env: { ...(command.env ?? {}) },
      });
    }
  }
  return { steps, skipped };
}

export function parseArguments(argv) {
  const options = {
    workspaces: undefined,
    fmt: false,
    noAnalyticsCheck: false,
    dryRun: false,
  };
  for (let index = 0; index < argv.length; index += 1) {
    const argument = argv[index];
    if (argument === "--workspaces") {
      if (index + 1 >= argv.length) {
        throw new Error("--workspaces needs a value");
      }
      options.workspaces = argv[++index];
    } else if (argument.startsWith("--workspaces=")) {
      options.workspaces = argument.slice("--workspaces=".length);
    } else if (argument === "--fmt") {
      options.fmt = true;
    } else if (argument === "--no-analytics-check") {
      options.noAnalyticsCheck = true;
    } else if (argument === "--dry-run") {
      options.dryRun = true;
    } else {
      throw new Error(`Unknown argument: ${argument}`);
    }
  }
  if (options.workspaces === undefined) {
    throw new Error("--workspaces is required (it can be empty)");
  }
  return options;
}

function formatCommand(step) {
  const env = Object.entries(step.env).map(
    ([name, value]) => `${name}=${JSON.stringify(value)}`,
  );
  return [...env, "cargo", ...step.args].join(" ");
}

function formatSeconds(milliseconds) {
  return `${(milliseconds / 1000).toFixed(1)}s`;
}

function main() {
  let options;
  let plan;
  try {
    options = parseArguments(process.argv.slice(2));
    plan = planRustChecks(options);
  } catch (error) {
    console.error(`::error::${error.message}`);
    process.exit(2);
  }

  for (const { workspace, reason } of plan.skipped) {
    console.log(`::notice::Skip Rust workspace ${workspace}: ${reason}.`);
  }
  if (plan.steps.length === 0) {
    console.log("::notice::No Rust workspace to check.");
    return;
  }

  const results = [];
  for (const step of plan.steps) {
    const title = `${step.workspace}: ${step.step}`;
    console.log(`::group::${title}`);
    console.log(`[${step.cwd}] ${formatCommand(step)}`);
    if (options.dryRun) {
      console.log("::endgroup::");
      results.push({ title, status: "dry run", milliseconds: 0 });
      continue;
    }
    const started = Date.now();
    const child = spawnSync(step.program, step.args, {
      cwd: join(REPO_ROOT, step.cwd),
      env: { ...process.env, ...step.env },
      shell: false,
      stdio: "inherit",
    });
    const milliseconds = Date.now() - started;
    console.log("::endgroup::");
    let status = "pass";
    if (child.error) {
      status = "fail";
      console.log(`::error::${title} did not start: ${child.error.message}`);
    } else if (child.status !== 0) {
      status = "fail";
      const cause =
        child.status === null
          ? `signal ${child.signal}`
          : `exit code ${child.status}`;
      console.log(`::error::${title} failed with ${cause}.`);
    }
    results.push({ title, status, milliseconds });
  }

  const width = Math.max(...results.map((result) => result.title.length));
  console.log("\nRust check summary:");
  for (const result of results) {
    console.log(
      `  ${result.title.padEnd(width)}  ${result.status.padEnd(7)}  ${formatSeconds(result.milliseconds)}`,
    );
  }
  for (const { workspace } of plan.skipped) {
    console.log(`  ${`${workspace}: all`.padEnd(width)}  skipped`);
  }
  const failed = results.filter((result) => result.status === "fail");
  if (failed.length > 0) {
    console.log(`::error::${failed.length} Rust check(s) failed.`);
    process.exitCode = 1;
  }
}

if (
  process.argv[1] &&
  import.meta.url === pathToFileURL(process.argv[1]).href
) {
  main();
}
