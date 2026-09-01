#!/usr/bin/env node

import { spawn as nodeSpawn } from "node:child_process";
import { constants as fsConstants, existsSync } from "node:fs";
import {
  access,
  lstat,
  mkdir,
  mkdtemp,
  readFile,
  realpath,
  rm,
  writeFile,
} from "node:fs/promises";
import { homedir, platform, release, tmpdir } from "node:os";
import { dirname, isAbsolute, join, resolve } from "node:path";
import { performance } from "node:perf_hooks";
import { pathToFileURL } from "node:url";

export const REPORT_SCHEMA_VERSION = 2;
export const PROFILE_MARKER = ".antiburn-memory-profile-v2";
export const DIAGNOSTIC_PREFIX = "@antiburn-mem ";
export const STEVE_UPSTREAM = "https://github.com/mikker/steve";
const MEMORY_BUNDLE_ID = "ai.antiburn.desktop.memory-probe";
const LAUNCH_ENVIRONMENT_KEYS = [
  "HOME",
  "CFFIXED_USER_HOME",
  "TMPDIR",
  "XDG_DATA_HOME",
  "XDG_CONFIG_HOME",
  "XDG_STATE_HOME",
  "ANTIBURN_ANALYTICS_ENABLED",
  "ANTIBURN_MEMORY_SESSIONS",
  "ANTIBURN_MEMORY_FIXTURE_SEED",
  "RUST_BACKTRACE",
  "RUST_LOG",
];

const PHASES = Object.freeze([
  "profile-created",
  "onboarding-started",
  "onboarding-complete",
  "measured-process-started",
  "shell-idle",
  "popover-open-requested",
  "popover-content-ready",
  "popover-visible-settled",
  "popover-hidden",
  "process-exited",
]);

function argumentError(message) {
  const error = new Error(message);
  error.code = "ERR_ARGUMENT";
  return error;
}

export function parseDuration(value, name = "duration") {
  const match = /^(\d+(?:\.\d+)?)(ms|s|m)$/.exec(String(value));
  if (!match) throw argumentError(`${name} requires a unit: ms, s, or m`);
  const factor = { ms: 1, s: 1_000, m: 60_000 }[match[2]];
  const milliseconds = Number(match[1]) * factor;
  if (!Number.isSafeInteger(milliseconds) || milliseconds <= 0) {
    throw argumentError(
      `${name} must be a positive whole number of milliseconds`,
    );
  }
  return milliseconds;
}

function integer(
  value,
  name,
  { minimum = 1, maximum = Number.MAX_SAFE_INTEGER } = {},
) {
  if (!/^-?\d+$/.test(String(value))) {
    throw argumentError(`${name} must be an integer`);
  }
  const number = Number(value);
  if (!Number.isSafeInteger(number) || number < minimum || number > maximum) {
    throw argumentError(`${name} must be between ${minimum} and ${maximum}`);
  }
  return number;
}

const OPTION_DEFINITIONS = {
  app: { key: "app" },
  runs: { key: "runs", parse: (value) => integer(value, "--runs") },
  samples: { key: "samples", parse: (value) => integer(value, "--samples") },
  "sample-interval": {
    key: "sampleIntervalMs",
    parse: (value) => parseDuration(value, "--sample-interval"),
  },
  settle: {
    key: "settleMs",
    parse: (value) => parseDuration(value, "--settle"),
  },
  timeout: {
    key: "timeoutMs",
    parse: (value) => parseDuration(value, "--timeout"),
  },
  metric: { key: "metric" },
  format: { key: "format" },
  output: { key: "output" },
  summary: { key: "summary" },
  "profile-root": { key: "profileRoot" },
  "keep-profile": { key: "keepProfile" },
  sessions: {
    key: "sessions",
    parse: (value) =>
      integer(value, "--sessions", { minimum: 0, maximum: 500 }),
  },
  "fixture-seed": {
    key: "fixtureSeed",
    parse: (value) => integer(value, "--fixture-seed", { minimum: 0 }),
  },
  steve: { key: "steve" },
};

export function parseArguments(arguments_) {
  const options = {
    release: false,
    noBuild: false,
    runs: 1,
    samples: 5,
    sampleIntervalMs: 250,
    settleMs: 2_000,
    timeoutMs: 30_000,
    metric: "both",
    format: "table",
    keepProfile: "failure",
    sessions: 225,
    fixtureSeed: 237,
    steve: "steve",
    quiet: false,
    help: false,
  };
  const seen = new Set();
  for (let index = 0; index < arguments_.length; index += 1) {
    const argument = arguments_[index];
    if (!argument.startsWith("--") || argument === "--") {
      throw argumentError(`unexpected argument: ${argument}`);
    }
    const [rawName, inlineValue] = argument.slice(2).split(/=(.*)/s, 2);
    if (["release", "no-build", "quiet", "help"].includes(rawName)) {
      if (inlineValue !== undefined) {
        throw argumentError(`--${rawName} does not take a value`);
      }
      if (seen.has(rawName)) {
        throw argumentError(`--${rawName} was provided more than once`);
      }
      seen.add(rawName);
      options[
        {
          release: "release",
          "no-build": "noBuild",
          quiet: "quiet",
          help: "help",
        }[rawName]
      ] = true;
      continue;
    }
    const definition = OPTION_DEFINITIONS[rawName];
    if (!definition) throw argumentError(`unknown option: --${rawName}`);
    if (seen.has(rawName)) {
      throw argumentError(`--${rawName} was provided more than once`);
    }
    const value = inlineValue ?? arguments_[++index];
    if (value === undefined || value.startsWith("--")) {
      throw argumentError(`--${rawName} requires a value`);
    }
    seen.add(rawName);
    options[definition.key] = definition.parse
      ? definition.parse(value)
      : value;
  }
  if (options.help) return options;
  if (options.release && !seen.has("runs")) options.runs = 5;
  if (!new Set(["rss", "footprint", "both"]).has(options.metric)) {
    throw argumentError("--metric must be rss, footprint, or both");
  }
  if (!new Set(["table", "json", "ndjson", "csv"]).has(options.format)) {
    throw argumentError("--format must be table, json, ndjson, or csv");
  }
  if (!new Set(["never", "failure", "always"]).has(options.keepProfile)) {
    throw argumentError("--keep-profile must be never, failure, or always");
  }
  if (options.app !== undefined) options.app = resolve(options.app);
  if (options.app !== undefined && options.release) {
    throw argumentError("--app conflicts with --release");
  }
  if (options.profileRoot !== undefined) {
    options.profileRoot = validateProfileRoot(options.profileRoot);
  }
  if (options.output !== undefined) options.output = resolve(options.output);
  if (options.summary !== undefined) options.summary = resolve(options.summary);
  if (options.output !== undefined && options.output === options.summary) {
    throw argumentError("--output and --summary must use different paths");
  }
  return options;
}

export function validateProfileRoot(
  path,
  { home = homedir(), temporary = tmpdir() } = {},
) {
  if (!isAbsolute(path)) {
    throw argumentError("--profile-root must be an absolute path");
  }
  const root = resolve(path);
  if (["/", resolve(home), resolve(temporary)].includes(root)) {
    throw argumentError(`unsafe profile root: ${root}`);
  }
  const known = [
    join(
      resolve(home),
      "Library",
      "Application Support",
      "ai.antiburn.desktop",
    ),
    join(resolve(home), ".config", "antiburn"),
    join(resolve(home), ".local", "share", "antiburn"),
  ];
  if (
    known.some(
      (item) =>
        root === item ||
        item.startsWith(`${root}/`) ||
        root.startsWith(`${item}/`),
    )
  ) {
    throw argumentError(
      `profile root overlaps an application profile: ${root}`,
    );
  }
  return root;
}

export async function createRunProfile({ root, run }) {
  const safeRoot = validateProfileRoot(
    root ?? join(tmpdir(), "antiburn-memory-report"),
  );
  await mkdir(safeRoot, { recursive: true, mode: 0o700 });
  const resolvedRoot = validateProfileRoot(await realpath(safeRoot));
  const scenarioRoot = join(resolvedRoot, "popover");
  await mkdir(scenarioRoot, { recursive: true, mode: 0o700 });
  const stat = await lstat(scenarioRoot);
  if (stat.isSymbolicLink() || !stat.isDirectory()) {
    throw new Error(`unsafe profile directory: ${scenarioRoot}`);
  }
  const runPath = await mkdtemp(
    join(scenarioRoot, `run-${String(run).padStart(3, "0")}-`),
  );
  await writeFile(join(runPath, PROFILE_MARKER), `${REPORT_SCHEMA_VERSION}\n`, {
    flag: "wx",
    mode: 0o600,
  });
  const directories = Object.fromEntries(
    ["home", "temp", "data", "config", "state"].map((name) => [
      name,
      join(runPath, name),
    ]),
  );
  await Promise.all(
    Object.values(directories).map((path) => mkdir(path, { mode: 0o700 })),
  );
  return { path: runPath, ...directories };
}

export function profileEnvironment(profile, base = process.env) {
  return {
    ...base,
    HOME: profile.home,
    CFFIXED_USER_HOME: profile.home,
    TMPDIR: profile.temp,
    XDG_DATA_HOME: profile.data,
    XDG_CONFIG_HOME: profile.config,
    XDG_STATE_HOME: profile.state,
    ANTIBURN_ANALYTICS_ENABLED: "false",
  };
}

export async function removeRunProfile(path) {
  const stat = await lstat(path);
  if (stat.isSymbolicLink() || !stat.isDirectory()) {
    throw new Error(`refusing to remove non-directory profile: ${path}`);
  }
  const resolved = await realpath(path);
  const marker = await readFile(join(resolved, PROFILE_MARKER), "utf8").catch(
    () => null,
  );
  if (marker !== `${REPORT_SCHEMA_VERSION}\n`) {
    throw new Error(`refusing to remove unmarked profile: ${path}`);
  }
  await rm(resolved, { recursive: true });
}

export function shouldKeepProfile(policy, failed) {
  return policy === "always" || (policy === "failure" && failed);
}

export function parseDiagnosticLine(line) {
  if (!line.startsWith(DIAGNOSTIC_PREFIX)) return null;
  let value;
  try {
    value = JSON.parse(line.slice(DIAGNOSTIC_PREFIX.length));
  } catch (error) {
    throw new Error(`invalid memory diagnostic JSON: ${error.message}`);
  }
  if (!value || typeof value.event !== "string") {
    throw new Error("invalid memory diagnostic envelope");
  }
  if (
    value.event === "webcontent" &&
    (!Number.isSafeInteger(value.pid) ||
      value.pid <= 0 ||
      !Number.isSafeInteger(value.generation) ||
      value.generation <= 0 ||
      value.window !== "popover")
  ) {
    throw new Error("invalid WebContent diagnostic");
  }
  return value;
}

export class LineCapture {
  #buffer = "";
  #lines = [];
  #offsets;

  constructor(paths = []) {
    this.#offsets = new Map(paths.map((path) => [path, 0]));
  }

  accept(chunk) {
    this.#buffer += chunk;
    for (;;) {
      const newline = this.#buffer.indexOf("\n");
      if (newline < 0) break;
      const line = this.#buffer.slice(0, newline).replace(/\r$/, "");
      this.#buffer = this.#buffer.slice(newline + 1);
      this.#lines.push(line);
    }
  }

  get lines() {
    return [...this.#lines];
  }

  async #readFiles() {
    for (const [path, offset] of this.#offsets) {
      const contents = await readFile(path, "utf8").catch((error) => {
        if (error.code === "ENOENT") return "";
        throw error;
      });
      if (contents.length > offset) {
        this.accept(contents.slice(offset));
        this.#offsets.set(path, contents.length);
      }
    }
  }

  async waitFor(predicate, timeoutMs) {
    const find = () => this.#lines.map(parseDiagnosticLine).find(predicate);
    const deadline = Date.now() + timeoutMs;
    while (Date.now() < deadline) {
      await this.#readFiles();
      const found = find();
      if (found) return found;
      await delay(50);
    }
    throw new Error(`memory diagnostic timed out after ${timeoutMs}ms`);
  }
}

export function spawnApplication(
  executable,
  {
    env = process.env,
    spawn = nodeSpawn,
    outputId = `${process.pid}-${performance.now()}`,
  } = {},
) {
  const application = resolve(dirname(executable), "../..");
  const outputRoot = env.TMPDIR ?? tmpdir();
  const stdoutPath = join(outputRoot, `antiburn-memory-${outputId}-stdout.log`);
  const stderrPath = join(outputRoot, `antiburn-memory-${outputId}-stderr.log`);
  const environment = LAUNCH_ENVIRONMENT_KEYS.flatMap((key) =>
    env[key] === undefined ? [] : ["--env", `${key}=${env[key]}`],
  );
  const child = spawn(
    "/usr/bin/open",
    [
      "-W",
      "-n",
      "-o",
      stdoutPath,
      "--stderr",
      stderrPath,
      ...environment,
      application,
    ],
    {
      env,
      stdio: ["ignore", "pipe", "pipe"],
    },
  );
  const capture = new LineCapture([stdoutPath, stderrPath]);
  for (const stream of [child.stdout, child.stderr]) {
    stream.setEncoding("utf8");
    stream.on("data", (chunk) => capture.accept(chunk));
  }
  return { child, capture };
}

export async function runCommand(
  file,
  arguments_,
  { spawn = nodeSpawn, timeoutMs = 30_000, env = process.env } = {},
) {
  return new Promise((resolvePromise, reject) => {
    const child = spawn(file, arguments_, {
      env,
      stdio: ["ignore", "pipe", "pipe"],
    });
    let stdout = "";
    let stderr = "";
    child.stdout.setEncoding("utf8");
    child.stderr.setEncoding("utf8");
    child.stdout.on("data", (chunk) => {
      stdout += chunk;
    });
    child.stderr.on("data", (chunk) => {
      stderr += chunk;
    });
    const timer = setTimeout(() => {
      child.kill("SIGKILL");
      reject(
        Object.assign(new Error(`${file} timed out after ${timeoutMs}ms`), {
          stdout,
          stderr,
        }),
      );
    }, timeoutMs);
    child.on("error", (error) => {
      clearTimeout(timer);
      reject(error);
    });
    child.on("close", (code, signal) => {
      clearTimeout(timer);
      if (code === 0) resolvePromise({ stdout, stderr });
      else {
        reject(
          Object.assign(new Error(`${file} exited with ${code ?? signal}`), {
            stdout,
            stderr,
            exitCode: code,
            signal,
          }),
        );
      }
    });
  });
}

export function parseSteveOutput(output) {
  let envelope;
  try {
    envelope = JSON.parse(output);
  } catch (error) {
    throw new Error(`Steve returned invalid JSON: ${error.message}`);
  }
  if (envelope?.ok !== true) {
    throw new Error(envelope?.error ?? "Steve command failed");
  }
  return envelope.data;
}

async function runSteve(options, arguments_, timeoutMs = options.timeoutMs) {
  let result;
  try {
    result = await runCommand(
      options.steve,
      [...arguments_, "--format", "json"],
      { timeoutMs },
    );
  } catch (error) {
    if (error.exitCode === 4) {
      throw new Error(
        "Steve needs macOS Accessibility permission. Enable it in System Settings > Privacy & Security > Accessibility.",
      );
    }
    if (error.code === "ENOENT") {
      throw new Error(
        `Steve is required. Install it from ${STEVE_UPSTREAM} with: brew tap mikker/tap && brew install steve`,
      );
    }
    throw Object.assign(
      new Error(error.stderr?.trim() || error.stdout?.trim() || error.message),
      { cause: error },
    );
  }
  return parseSteveOutput(result.stdout);
}

export function findStatusItem(tree) {
  const matches = [];
  const visit = (node) => {
    if (!node || typeof node !== "object") return;
    const frame = node.frame;
    if (
      node.role === "AXMenuBarItem" &&
      Number(frame?.width) > 0 &&
      Number(frame?.height) > 0 &&
      Number(frame?.y) < 40
    ) {
      matches.push(node);
    }
    for (const child of node.children ?? []) visit(child);
  };
  for (const root of Array.isArray(tree) ? tree : [tree]) visit(root);
  if (matches.length !== 1) {
    throw new Error(
      `expected one antiburn status item; found ${matches.length}. Close other antiburn instances and make the item visible.`,
    );
  }
  return matches[0];
}

function frameCenter(item) {
  return {
    x: item.frame.x + item.frame.width / 2,
    y: item.frame.y + item.frame.height / 2,
  };
}

async function statusItem(options, pid) {
  const deadline = Date.now() + options.timeoutMs;
  let lastError;
  while (Date.now() < deadline) {
    const tree = await runSteve(options, [
      "elements",
      "--pid",
      String(pid),
      "--depth",
      "3",
    ]);
    try {
      return findStatusItem(tree);
    } catch (error) {
      if (!error.message.includes("found 0")) throw error;
      lastError = error;
      await delay(100);
    }
  }
  throw lastError ?? new Error("antiburn status item did not appear");
}

async function clickStatusItem(options, item, right = false) {
  const point = frameCenter(item);
  await runSteve(options, [
    "click-at",
    String(point.x),
    String(point.y),
    ...(right ? ["--right"] : []),
  ]);
}

async function waitForText(options, pid, text) {
  await runSteve(options, [
    "wait",
    "--pid",
    String(pid),
    "--text",
    text,
    "--timeout",
    String(Math.ceil(options.timeoutMs / 1_000)),
  ]);
}

async function waitForTextGone(options, pid, text) {
  await runSteve(options, [
    "wait",
    "--pid",
    String(pid),
    "--text",
    text,
    "--gone",
    "--timeout",
    String(Math.ceil(options.timeoutMs / 1_000)),
  ]);
}

async function waitForApp(options, pid) {
  const deadline = Date.now() + options.timeoutMs;
  let lastError;
  while (Date.now() < deadline) {
    try {
      return await runSteve(
        options,
        ["resolve", "--pid", String(pid)],
        Math.min(5_000, options.timeoutMs),
      );
    } catch (error) {
      lastError = error;
      await delay(200);
    }
  }
  throw new Error(
    `Steve could not attach to application ${pid}: ${lastError?.message ?? "not found"}`,
  );
}

async function waitForLaunchedPid(options, excludedPids = new Set()) {
  const deadline = Date.now() + options.timeoutMs;
  while (Date.now() < deadline) {
    const matches = (await runSteve(options, ["apps"])).filter(
      (app) => app.bundleId === MEMORY_BUNDLE_ID && !excludedPids.has(app.pid),
    );
    if (matches.length === 1) return matches[0].pid;
    if (matches.length > 1) {
      throw new Error(
        `expected one memory probe application; found ${matches.length}`,
      );
    }
    await delay(100);
  }
  throw new Error("memory probe application did not launch");
}

async function existingProbePids(options) {
  return new Set(
    (await runSteve(options, ["apps"]))
      .filter((app) => app.bundleId === MEMORY_BUNDLE_ID)
      .map((app) => app.pid),
  );
}

async function waitAndClick(options, pid, title) {
  const deadline = Date.now() + options.timeoutMs;
  let lastError;
  while (Date.now() < deadline) {
    try {
      const matches = await runSteve(
        options,
        ["find", "--pid", String(pid), "--title", title],
        Math.min(5_000, options.timeoutMs),
      );
      const target = matches.find(
        (item) =>
          Number(item.frame?.width) > 0 && Number(item.frame?.height) > 0,
      );
      if (target) {
        const point = frameCenter(target);
        await runSteve(options, ["click-at", String(point.x), String(point.y)]);
        return;
      }
    } catch (error) {
      lastError = error;
    }
    await delay(200);
  }
  throw new Error(
    `Steve could not click ${JSON.stringify(title)}: ${lastError?.message ?? "element not found"}`,
  );
}

async function quitFromTray(options, pid, item) {
  await clickStatusItem(options, item, true);
  await runSteve(options, [
    "click",
    "--pid",
    String(pid),
    "--title",
    "Quit antiburn",
  ]);
}

export function parsePsRss(output, expectedPids = []) {
  const rows = [];
  for (const line of output.split(/\r?\n/)) {
    if (!line.trim()) continue;
    const match = /^\s*(\d+)\s+(\d+)\s+(.+?)\s*$/.exec(line);
    if (!match) {
      throw Object.assign(new Error(`could not parse ps RSS output: ${line}`), {
        rawOutput: output,
      });
    }
    rows.push({
      pid: Number(match[1]),
      rssBytes: Number(match[2]) * 1024,
      startedAt: match[3],
    });
  }
  verifyExpectedPids(rows, expectedPids, output);
  return rows;
}

function verifyExpectedPids(rows, expectedPids, rawOutput) {
  const expected = [...new Set(expectedPids)].sort((a, b) => a - b);
  const actual = rows.map((row) => row.pid).sort((a, b) => a - b);
  if (
    expected.length &&
    (expected.length !== actual.length ||
      expected.some((pid, index) => pid !== actual[index]))
  ) {
    throw Object.assign(
      new Error(
        `ps returned PIDs ${actual.join(", ")}; expected ${expected.join(", ")}`,
      ),
      { rawOutput },
    );
  }
}

function parseMemoryValue(number, unit = "B") {
  const normalized = unit.toUpperCase().replace("IB", "B");
  const factor = {
    B: 1,
    K: 2 ** 10,
    KB: 2 ** 10,
    M: 2 ** 20,
    MB: 2 ** 20,
    G: 2 ** 30,
    GB: 2 ** 30,
  }[normalized];
  if (!factor) throw new Error(`unsupported memory unit: ${unit}`);
  const bytes = Number(number.replaceAll(",", "")) * factor;
  if (!Number.isSafeInteger(bytes) || bytes < 0) {
    throw new Error(`invalid memory value: ${number} ${unit}`);
  }
  return bytes;
}

export function parseFootprint(output, expectedPids = []) {
  const processes = [];
  let current = null;
  let aggregatePhysicalFootprintBytes = null;
  for (const line of output.split(/\r?\n/)) {
    const processMatch =
      /^(?:\s*(?:Process|Footprint for process)\s*:?\s*.*?)?[^[]*\[(\d+)](?::.*Footprint:.*)?\s*$/i.exec(
        line,
      );
    if (processMatch) {
      current = { pid: Number(processMatch[1]), physicalFootprintBytes: null };
      processes.push(current);
      continue;
    }
    const physicalMatch =
      /^\s*phys_footprint\s*[:=]\s*([\d,.]+)\s*([KMGT]?i?B|[KMGT])?\s*$/i.exec(
        line,
      );
    if (physicalMatch && current) {
      current.physicalFootprintBytes = parseMemoryValue(
        physicalMatch[1],
        physicalMatch[2] ?? "B",
      );
      continue;
    }
    const aggregateMatch =
      /^\s*(?:(?:Summary\s+)?TOTAL(?:\s+physical footprint)?|Summary Footprint)\s*[:=]?\s*([\d,.]+)\s*([KMGT]?i?B|[KMGT])\s*$/i.exec(
        line,
      );
    if (aggregateMatch) {
      aggregatePhysicalFootprintBytes = parseMemoryValue(
        aggregateMatch[1],
        aggregateMatch[2],
      );
    }
  }
  const expected = [...new Set(expectedPids)];
  const found = processes
    .filter((item) => item.physicalFootprintBytes !== null)
    .map((item) => item.pid);
  if (
    aggregatePhysicalFootprintBytes === null &&
    expected.length === 1 &&
    processes.length === 1 &&
    processes[0].physicalFootprintBytes !== null
  ) {
    aggregatePhysicalFootprintBytes = processes[0].physicalFootprintBytes;
  }
  if (
    expected.some((pid) => !found.includes(pid)) ||
    aggregatePhysicalFootprintBytes === null
  ) {
    throw Object.assign(
      new Error(
        "could not parse all footprint process ledgers and the deduplicated TOTAL",
      ),
      { rawOutput: output },
    );
  }
  return { processes, aggregatePhysicalFootprintBytes, rawOutput: output };
}

export async function processIdentities(
  pids,
  { command = runCommand, timeoutMs = 30_000 } = {},
) {
  const { stdout } = await command(
    "/bin/ps",
    ["-o", "pid=,rss=,lstart=", "-p", [...new Set(pids)].join(",")],
    { timeoutMs },
  );
  return parsePsRss(stdout, pids).map(
    ({ rssBytes: _, ...identity }) => identity,
  );
}

export async function collectMemory(
  processes,
  metric,
  { command = runCommand, timeoutMs = 30_000 } = {},
) {
  const pids = processes.map((item) => item.pid);
  const result = {
    processes: processes.map((process) => ({ ...process, memory: {} })),
  };
  if (metric === "rss" || metric === "both") {
    const { stdout } = await command(
      "/bin/ps",
      ["-o", "pid=,rss=,lstart=", "-p", pids.join(",")],
      { timeoutMs },
    );
    const rows = parsePsRss(stdout, pids);
    for (const target of result.processes) {
      const row = rows.find((item) => item.pid === target.pid);
      if (row.startedAt !== target.startedAt) {
        throw new Error(`process ${target.pid} start identity changed`);
      }
      target.memory.rssBytes = row.rssBytes;
    }
    result.rssSumBytes = result.processes.reduce(
      (sum, item) => sum + item.memory.rssBytes,
      0,
    );
  }
  if (metric === "footprint" || metric === "both") {
    const { stdout } = await command(
      "/usr/bin/footprint",
      ["-f", "bytes", ...pids.flatMap((pid) => ["-p", String(pid)])],
      { timeoutMs },
    );
    const parsed = parseFootprint(stdout, pids);
    for (const target of result.processes) {
      target.memory.physicalFootprintBytes = parsed.processes.find(
        (item) => item.pid === target.pid,
      ).physicalFootprintBytes;
    }
    result.aggregatePhysicalFootprintBytes =
      parsed.aggregatePhysicalFootprintBytes;
  }
  const after = await processIdentities(pids, { command, timeoutMs });
  for (const process of processes) {
    if (
      after.find((item) => item.pid === process.pid)?.startedAt !==
      process.startedAt
    ) {
      throw new Error(
        `process ${process.pid} identity changed during sampling`,
      );
    }
  }
  return result;
}

export function statistics(values) {
  if (values.length === 0) return null;
  const sorted = [...values].sort((a, b) => a - b);
  const mean = sorted.reduce((sum, value) => sum + value, 0) / sorted.length;
  const middle = Math.floor(sorted.length / 2);
  const median =
    sorted.length % 2
      ? sorted[middle]
      : (sorted[middle - 1] + sorted[middle]) / 2;
  const variance =
    sorted.reduce((sum, value) => sum + (value - mean) ** 2, 0) / sorted.length;
  return {
    count: sorted.length,
    minimum: sorted[0],
    median,
    p95: sorted[Math.max(0, Math.ceil(sorted.length * 0.95) - 1)],
    maximum: sorted.at(-1),
    mean,
    standardDeviation: Math.sqrt(variance),
  };
}

export function summarizeSamples(samples) {
  const groups = new Map();
  for (const sample of samples) {
    for (const [metric, value] of Object.entries(sample.memory)) {
      const key = JSON.stringify([sample.run, sample.process.role, metric]);
      const group = groups.get(key) ?? {
        run: sample.run,
        scenario: "popover",
        phase: "popover-visible-settled",
        role: sample.process.role,
        metric,
        values: [],
      };
      group.values.push(value);
      groups.set(key, group);
    }
  }
  const runs = [...groups.values()].map(({ values, ...group }) => ({
    ...group,
    ...statistics(values),
  }));
  const acrossGroups = new Map();
  for (const summary of runs) {
    const key = JSON.stringify([summary.role, summary.metric]);
    const group = acrossGroups.get(key) ?? {
      scenario: "popover",
      phase: "popover-visible-settled",
      role: summary.role,
      metric: summary.metric,
      medians: [],
      peaks: [],
    };
    group.medians.push(summary.median);
    group.peaks.push(summary.maximum);
    acrossGroups.set(key, group);
  }
  const acrossRuns = [...acrossGroups.values()].map(
    ({ medians, peaks, ...group }) => ({
      ...group,
      runMedianSummary: statistics(medians),
      runPeakSummary: statistics(peaks),
    }),
  );
  return { runs, acrossRuns };
}

export function recordPhase(report, run, phase, details = {}) {
  if (!PHASES.includes(phase))
    throw new Error(`unknown popover phase: ${phase}`);
  const value = {
    run,
    scenario: "popover",
    phase,
    wallTime: new Date().toISOString(),
    monotonicMs: performance.now(),
    ...details,
  };
  report.phaseTimings.push(value);
  return value;
}

function countAccessibilityNodes(tree) {
  let nodes = 0;
  let syntheticSessionLabels = 0;
  const visit = (node) => {
    if (!node || typeof node !== "object") return;
    nodes += 1;
    if (
      [node.title, node.value, node.description].some((value) =>
        String(value ?? "").includes("Synthetic coding session"),
      )
    ) {
      syntheticSessionLabels += 1;
    }
    for (const child of node.children ?? []) visit(child);
  };
  for (const root of tree) visit(root);
  return { accessibilityNodes: nodes, syntheticSessionLabels };
}

async function sampleVisiblePopover(context, processes) {
  const tree = await runSteve(context.options, [
    "elements",
    "--pid",
    String(context.pid),
    "--depth",
    "8",
  ]);
  const dimensions = countAccessibilityNodes(tree);
  await delay(context.options.settleMs);
  for (let sequence = 1; sequence <= context.options.samples; sequence += 1) {
    const collected = await collectMemory(processes, context.options.metric, {
      timeoutMs: context.options.timeoutMs,
    });
    for (const process of collected.processes) {
      context.report.samples.push({
        run: context.run,
        scenario: "popover",
        phase: "popover-visible-settled",
        sequence,
        wallTime: new Date().toISOString(),
        monotonicMs: performance.now(),
        process: Object.fromEntries(
          Object.entries(process).filter(([key]) => key !== "memory"),
        ),
        memory: process.memory,
        dimensions,
      });
    }
    context.report.samples.push({
      run: context.run,
      scenario: "popover",
      phase: "popover-visible-settled",
      sequence,
      wallTime: new Date().toISOString(),
      monotonicMs: performance.now(),
      process: {
        role: "application-total",
        pids: collected.processes.map((process) => process.pid),
      },
      memory: Object.fromEntries(
        Object.entries({
          rssSumBytes: collected.rssSumBytes,
          aggregatePhysicalFootprintBytes:
            collected.aggregatePhysicalFootprintBytes,
        }).filter(([, value]) => value !== undefined),
      ),
      dimensions,
    });
    if (sequence < context.options.samples) {
      await delay(context.options.sampleIntervalMs);
    }
  }
}

function delay(milliseconds) {
  return new Promise((resolvePromise) =>
    setTimeout(resolvePromise, milliseconds),
  );
}

function killIfRunning(pid) {
  try {
    process.kill(pid, "SIGKILL");
  } catch (error) {
    if (error.code !== "ESRCH") throw error;
  }
}

function waitForExit(child, timeoutMs) {
  if (child.exitCode !== null || child.signalCode !== null) {
    return Promise.resolve({ code: child.exitCode, signal: child.signalCode });
  }
  return new Promise((resolvePromise, reject) => {
    const timer = setTimeout(
      () => reject(new Error(`application did not exit after ${timeoutMs}ms`)),
      timeoutMs,
    );
    child.once("exit", (code, signal) => {
      clearTimeout(timer);
      resolvePromise({ code, signal });
    });
  });
}

async function prepareProfile(options, executable, profile, report, run) {
  const excludedPids = await existingProbePids(options);
  const launched = spawnApplication(executable, {
    env: profileEnvironment(profile),
  });
  let pid;
  let stage = "find preparation process";
  try {
    pid = await waitForLaunchedPid(options, excludedPids);
    recordPhase(report, run, "onboarding-started", { pid });
    stage = "attach to onboarding";
    await waitForApp(options, pid);
    stage = "wait for welcome";
    await waitForText(options, pid, "Stop hitting your token limits");
    stage = "continue from welcome";
    await waitAndClick(options, pid, "Continue");
    stage = "wait for repository setup";
    await waitForText(options, pid, "Repo search locations");
    stage = "continue from repository setup";
    await waitAndClick(options, pid, "Continue");
    stage = "wait for ready step";
    await waitForText(options, pid, "Ready");
    stage = "finish onboarding";
    await waitAndClick(options, pid, "Start using antiburn");
    stage = "wait for onboarding handoff";
    await waitForTextGone(options, pid, "Start using antiburn");
    recordPhase(report, run, "onboarding-complete");
    stage = "stop preparation launch";
    await delay(500);
    killIfRunning(pid);
    const exit = await waitForExit(launched.child, options.timeoutMs);
    if (exit.code !== 0) {
      throw new Error(
        `preparation launch exited with ${exit.code ?? exit.signal}`,
      );
    }
    return pid;
  } catch (error) {
    throw Object.assign(new Error(`${stage}: ${error.message}`), {
      cause: error,
      preparationDiagnostics: launched.capture.lines,
    });
  } finally {
    if (launched.child.exitCode === null) {
      if (pid) killIfRunning(pid);
      launched.child.kill("SIGKILL");
    }
  }
}

async function executableFor(options) {
  if (options.app) {
    await access(options.app, fsConstants.X_OK);
    return options.app;
  }
  const profile = options.release ? "release" : "debug";
  const executable = resolve(
    "apps/desktop/src-tauri/target",
    profile,
    "bundle/macos/antiburn-memory-probe.app/Contents/MacOS/antiburn",
  );
  if (options.noBuild) {
    if (!existsSync(executable)) {
      throw new Error(`memory executable does not exist: ${executable}`);
    }
  } else {
    await runCommand(
      "pnpm",
      [
        "--filter",
        "@antiburn/desktop",
        "exec",
        "tauri",
        "build",
        "--bundles",
        "app",
        "--features",
        "memory-probe",
        "--config",
        "src-tauri/tauri.memory-probe.conf.json",
        ...(!options.release ? ["--debug"] : []),
      ],
      { timeoutMs: 30 * 60_000 },
    );
  }
  await access(executable, fsConstants.X_OK);
  return executable;
}

async function verifySteve(options) {
  await runSteve(options, ["apps"]);
}

async function rejectRunningAntiburn(options) {
  const apps = await runSteve(options, ["apps"]);
  const running = apps.filter((app) =>
    String(app.bundleId).startsWith("ai.antiburn.desktop"),
  );
  if (running.length) {
    throw new Error(
      `close other antiburn instances before measuring: ${running.map((app) => app.pid).join(", ")}`,
    );
  }
}

export async function runReport(options) {
  if (platform() !== "darwin") {
    throw new Error("memory reporting requires macOS 13 or later");
  }
  await verifySteve(options);
  await rejectRunningAntiburn(options);
  const executable = await executableFor(options);
  const report = {
    schemaVersion: REPORT_SCHEMA_VERSION,
    configuration: { ...options, app: executable, scenario: "popover" },
    platform: {
      platform: platform(),
      osRelease: release(),
      node: process.version,
      steve: STEVE_UPSTREAM,
    },
    phaseTimings: [],
    samples: [],
    summaries: { runs: [], acrossRuns: [] },
    warnings: [],
    failures: [],
  };
  for (let run = 1; run <= options.runs; run += 1) {
    const profile = await createRunProfile({ root: options.profileRoot, run });
    let failed = false;
    let launched;
    recordPhase(report, run, "profile-created", { profile: profile.path });
    try {
      await prepareProfile(options, executable, profile, report, run);
      const excludedPids = await existingProbePids(options);
      launched = spawnApplication(executable, {
        env: profileEnvironment(profile, {
          ...process.env,
          ANTIBURN_MEMORY_SESSIONS: String(options.sessions),
          ANTIBURN_MEMORY_FIXTURE_SEED: String(options.fixtureSeed),
        }),
      });
      const pid = await waitForLaunchedPid(options, excludedPids);
      recordPhase(report, run, "measured-process-started", { pid });
      await waitForApp(options, pid);
      const [shell] = await processIdentities([pid], {
        timeoutMs: options.timeoutMs,
      });
      recordPhase(report, run, "shell-idle");
      const item = await statusItem(options, pid);
      recordPhase(report, run, "popover-open-requested", { frame: item.frame });
      await clickStatusItem(options, item);
      await waitForText(options, pid, "Sessions");
      const rendererEvent = await launched.capture.waitFor(
        (event) => event?.event === "webcontent" && event.window === "popover",
        options.timeoutMs,
      );
      recordPhase(report, run, "popover-content-ready", {
        generation: rendererEvent.generation,
        webContentPid: rendererEvent.pid,
      });
      const [renderer] = await processIdentities([rendererEvent.pid], {
        timeoutMs: options.timeoutMs,
      });
      const processes = [
        { role: "shell", ...shell },
        {
          role: "renderer",
          window: "popover",
          generation: rendererEvent.generation,
          ...renderer,
        },
      ];
      recordPhase(report, run, "popover-visible-settled");
      await sampleVisiblePopover({ options, report, run, pid }, processes);
      await clickStatusItem(options, item);
      await runSteve(options, [
        "wait",
        "--pid",
        String(pid),
        "--window-count",
        "0",
        "--timeout",
        String(Math.ceil(options.timeoutMs / 1_000)),
      ]);
      recordPhase(report, run, "popover-hidden");
      await quitFromTray(options, pid, item);
      const exit = await waitForExit(launched.child, options.timeoutMs);
      if (exit.code !== 0) {
        throw new Error(`application exited with ${exit.code ?? exit.signal}`);
      }
      recordPhase(report, run, "process-exited");
    } catch (error) {
      failed = true;
      report.failures.push({
        run,
        scenario: "popover",
        message: error.message,
        code: error.code,
        rawOutput: error.rawOutput,
        diagnostics: [
          ...(error.preparationDiagnostics ?? []),
          ...(launched?.capture.lines ?? []),
        ],
        profile: profile.path,
      });
    } finally {
      if (launched?.child.exitCode === null) {
        const matches = await runSteve(options, ["apps"]).catch(() => []);
        for (const app of matches) {
          if (app.bundleId === MEMORY_BUNDLE_ID) {
            killIfRunning(app.pid);
          }
        }
        launched.child.kill("SIGKILL");
      }
      if (!shouldKeepProfile(options.keepProfile, failed)) {
        await removeRunProfile(profile.path);
      }
    }
  }
  report.summaries = summarizeSamples(report.samples);
  return report;
}

function csvCell(value) {
  const text = value === undefined || value === null ? "" : String(value);
  return /[",\r\n]/.test(text) ? `"${text.replaceAll('"', '""')}"` : text;
}

export function formatReport(report, format) {
  if (format === "json") return `${JSON.stringify(report, null, 2)}\n`;
  if (format === "ndjson") {
    const records = [
      {
        type: "report",
        schemaVersion: report.schemaVersion,
        configuration: report.configuration,
        platform: report.platform,
      },
      ...report.phaseTimings.map((value) => ({ type: "phase", ...value })),
      ...report.samples.map((value) => ({ type: "sample", ...value })),
      ...report.summaries.runs.map((value) => ({
        type: "run-summary",
        ...value,
      })),
      ...report.summaries.acrossRuns.map((value) => ({
        type: "cross-run-summary",
        ...value,
      })),
      ...report.failures.map((value) => ({ type: "failure", ...value })),
    ];
    return `${records.map((record) => JSON.stringify(record)).join("\n")}\n`;
  }
  if (format === "csv") {
    const header = [
      "run",
      "phase",
      "sequence",
      "role",
      "pid",
      "rssBytes",
      "physicalFootprintBytes",
      "accessibilityNodes",
      "syntheticSessionLabels",
    ];
    const rows = report.samples.map((sample) => [
      sample.run,
      sample.phase,
      sample.sequence,
      sample.process.role,
      sample.process.pid,
      sample.memory.rssBytes,
      sample.memory.physicalFootprintBytes,
      sample.dimensions.accessibilityNodes,
      sample.dimensions.syntheticSessionLabels,
    ]);
    return `${[header, ...rows]
      .map((row) => row.map(csvCell).join(","))
      .join("\n")}\n`;
  }
  const heading =
    "Run Role              Metric                              Median       Peak";
  const lines = report.summaries.runs.map(
    (item) =>
      `${String(item.run).padStart(3)} ${item.role.padEnd(17)} ${item.metric.padEnd(34)} ${formatBytes(item.median).padStart(10)} ${formatBytes(item.maximum).padStart(10)}`,
  );
  if (report.failures.length) {
    lines.push(
      ...report.failures.map(
        (failure) => `FAIL run ${failure.run}: ${failure.message}`,
      ),
    );
  }
  return `${heading}\n${lines.join("\n")}\n`;
}

export function formatBytes(bytes) {
  if (!Number.isFinite(bytes)) return "-";
  for (const [unit, factor] of [
    ["GiB", 2 ** 30],
    ["MiB", 2 ** 20],
    ["KiB", 2 ** 10],
  ]) {
    if (bytes >= factor) return `${(bytes / factor).toFixed(1)} ${unit}`;
  }
  return `${bytes.toFixed(0)} B`;
}

export async function writeReport(report, { format, output, summary }) {
  const content = formatReport(report, format);
  if (output) await writeFile(output, content, { flag: "wx" });
  else process.stdout.write(content);
  if (summary) {
    await writeFile(
      summary,
      `${JSON.stringify(
        {
          schemaVersion: report.schemaVersion,
          summaries: report.summaries,
          failures: report.failures,
        },
        null,
        2,
      )}\n`,
      { flag: "wx" },
    );
  }
}

export function usage() {
  return `Usage: node scripts/mem-report.mjs [options]

Measures the settled antiburn popover with 225 deterministic rows by default.
Live measurements require macOS 13+, a logged-in GUI session, and Steve with
Accessibility permission. Install Steve with:
  brew tap mikker/tap && brew install steve

Options:
  --release                   Build and measure an optimized .app
  --app <path>                Use an existing probe-enabled executable
  --no-build                  Require the existing executable
  --runs <count>              Independent profiles and launches
  --samples <count>           Settled popover samples (default: 5)
  --sample-interval <time>    Delay between samples (default: 250ms)
  --settle <time>             Delay after content readiness (default: 2s)
  --timeout <time>            Per-action timeout (default: 30s)
  --metric <name>             rss, footprint, or both (default: both)
  --sessions <count>          Deterministic rows, 0 through 500 (default: 225)
  --fixture-seed <integer>    Deterministic fixture seed (default: 237)
  --steve <path>              Steve executable (default: steve from PATH)
  --format <name>             table, json, ndjson, or csv
  --output <path>             Report destination
  --summary <path>            Compact JSON summary destination
  --profile-root <path>       Parent for isolated profiles
  --keep-profile <policy>     never, failure, or always
  --quiet                     Hide successful diagnostics
  --help                      Print this help
`;
}

async function main() {
  let options;
  try {
    options = parseArguments(process.argv.slice(2));
    if (options.help) {
      process.stdout.write(usage());
      return;
    }
    const report = await runReport(options);
    await writeReport(report, options);
    if (report.failures.length) process.exitCode = 1;
  } catch (error) {
    process.stderr.write(`${error.message}\n`);
    process.exitCode = 1;
  }
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? "").href) {
  await main();
}
