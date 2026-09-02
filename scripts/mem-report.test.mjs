import assert from "node:assert/strict";
import { mkdtemp, mkdir, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { PassThrough } from "node:stream";
import test from "node:test";

import {
  DIAGNOSTIC_PREFIX,
  LineCapture,
  ONBOARDING_STEPS,
  PROFILE_MARKER,
  collectMemory,
  createRunProfile,
  findStatusItem,
  formatReport,
  parseArguments,
  parseDiagnosticLine,
  parseDuration,
  parseFootprint,
  parsePsRss,
  parseSteveOutput,
  profileEnvironment,
  recordPhase,
  removeRunProfile,
  shouldKeepProfile,
  spawnApplication,
  statistics,
  summarizeSamples,
  validateProfileRoot,
} from "./mem-report.mjs";

test("pins the current four-step onboarding path", () => {
  assert.deepEqual(ONBOARDING_STEPS, [
    { heading: "Stop hitting your token limits", action: "Continue" },
    { heading: "Scan Locations: Agents", action: "Continue" },
    { heading: "Scan Locations: Repos", action: "Continue" },
    { heading: "Ready", action: "Start using antiburn" },
  ]);
});

test("parses defaults, release defaults, and explicit options", () => {
  const defaults = parseArguments([]);
  assert.equal(defaults.sessions, 225);
  assert.equal(defaults.runs, 1);
  assert.equal(defaults.samples, 5);
  assert.equal(parseArguments(["--release"]).runs, 5);
  const explicit = parseArguments([
    "--release",
    "--runs",
    "2",
    "--sessions=0",
    "--metric",
    "rss",
    "--steve",
    "/opt/steve",
  ]);
  assert.equal(explicit.runs, 2);
  assert.equal(explicit.sessions, 0);
  assert.equal(explicit.steve, "/opt/steve");
  assert.throws(
    () => parseArguments(["--app", "app", "--release"]),
    /conflicts/,
  );
  assert.throws(() => parseArguments(["--sessions", "501"]), /between/);
  assert.throws(() => parseArguments(["--unknown"]), /unknown option/);
});

test("parses explicit duration units", () => {
  assert.equal(parseDuration("250ms"), 250);
  assert.equal(parseDuration("1.5s"), 1_500);
  assert.equal(parseDuration("2m"), 120_000);
  for (const value of ["250", "0s", "1 sec", "-1s"]) {
    assert.throws(() => parseDuration(value), /unit|positive/);
  }
});

test("rejects unsafe profile roots without filesystem access", () => {
  assert.throws(() => validateProfileRoot("relative/path"), /absolute/);
  assert.throws(() => validateProfileRoot("/"), /unsafe/);
  assert.throws(
    () =>
      validateProfileRoot("/Users/test", {
        home: "/Users/test",
        temporary: "/tmp",
      }),
    /unsafe/,
  );
  assert.throws(
    () =>
      validateProfileRoot("/Users/test/Library/Application Support", {
        home: "/Users/test",
        temporary: "/tmp",
      }),
    /overlaps/,
  );
});

test("creates isolated profiles and deletes only marked directories", async (t) => {
  const parent = await mkdtemp(join(tmpdir(), "mem-report-test-"));
  t.after(() => rm(parent, { recursive: true, force: true }));
  const profile = await createRunProfile({
    root: join(parent, "profiles"),
    run: 1,
  });
  assert.equal(
    await readFile(join(profile.path, PROFILE_MARKER), "utf8"),
    "2\n",
  );
  for (const key of ["home", "temp", "data", "config", "state"]) {
    assert.ok(profile[key].startsWith(profile.path));
  }
  const env = profileEnvironment(profile, { PATH: "/bin" });
  assert.equal(env.HOME, profile.home);
  assert.equal(env.ANTIBURN_ANALYTICS_ENABLED, "false");
  await removeRunProfile(profile.path);
  const unsafe = join(parent, "unsafe");
  await mkdir(unsafe);
  await assert.rejects(removeRunProfile(unsafe), /unmarked/);
});

test("applies profile retention policies", () => {
  assert.equal(shouldKeepProfile("never", true), false);
  assert.equal(shouldKeepProfile("failure", false), false);
  assert.equal(shouldKeepProfile("failure", true), true);
  assert.equal(shouldKeepProfile("always", false), true);
});

test("parses only prefixed WebContent diagnostics", () => {
  assert.equal(parseDiagnosticLine("normal log"), null);
  assert.deepEqual(
    parseDiagnosticLine(
      `${DIAGNOSTIC_PREFIX}{"event":"webcontent","window":"popover","generation":2,"pid":42}`,
    ),
    { event: "webcontent", window: "popover", generation: 2, pid: 42 },
  );
  assert.throws(
    () => parseDiagnosticLine(`${DIAGNOSTIC_PREFIX}not-json`),
    /invalid/,
  );
  assert.throws(
    () =>
      parseDiagnosticLine(
        `${DIAGNOSTIC_PREFIX}{"event":"webcontent","window":"popover","generation":0,"pid":42}`,
      ),
    /invalid WebContent/,
  );
});

test("line capture waits for a diagnostic across stream chunks", async () => {
  const capture = new LineCapture();
  const pending = capture.waitFor(
    (event) => event?.event === "webcontent",
    1_000,
  );
  capture.accept("ordinary log\n@antiburn-");
  capture.accept(
    'mem {"event":"webcontent","window":"popover","generation":1,"pid":9}\n',
  );
  assert.equal((await pending).pid, 9);
  assert.equal(capture.lines[0], "ordinary log");
});

test("launches the application bundle through macOS Launch Services", () => {
  let call;
  const child = {
    stdout: new PassThrough(),
    stderr: new PassThrough(),
  };
  const launched = spawnApplication(
    "/tmp/antiburn.app/Contents/MacOS/antiburn",
    {
      env: { HOME: "/tmp/home", TMPDIR: "/tmp", TEST: "1" },
      outputId: "test",
      spawn: (...arguments_) => {
        call = arguments_;
        return child;
      },
    },
  );
  assert.equal(launched.child, child);
  assert.deepEqual(call, [
    "/usr/bin/open",
    [
      "-W",
      "-n",
      "-o",
      "/tmp/antiburn-memory-test-stdout.log",
      "--stderr",
      "/tmp/antiburn-memory-test-stderr.log",
      "--env",
      "HOME=/tmp/home",
      "--env",
      "TMPDIR=/tmp",
      "/tmp/antiburn.app",
    ],
    {
      env: { HOME: "/tmp/home", TMPDIR: "/tmp", TEST: "1" },
      stdio: ["ignore", "pipe", "pipe"],
    },
  ]);
});

test("parses Steve envelopes and finds one framed status item", () => {
  assert.deepEqual(parseSteveOutput('{"ok":true,"data":[1]}'), [1]);
  assert.throws(
    () => parseSteveOutput('{"ok":false,"error":"denied"}'),
    /denied/,
  );
  assert.throws(() => parseSteveOutput("not json"), /invalid JSON/);
  const item = findStatusItem([
    {
      role: "AXApplication",
      children: [
        {
          role: "AXMenuBar",
          children: [
            {
              role: "AXMenuBarItem",
              frame: { x: 0, y: 30, width: 0, height: 0 },
            },
          ],
        },
        {
          role: "AXMenuBar",
          children: [
            {
              id: "ax://42/0.1.0",
              role: "AXMenuBarItem",
              frame: { x: 900, y: 3, width: 36, height: 24 },
            },
          ],
        },
      ],
    },
  ]);
  assert.equal(item.id, "ax://42/0.1.0");
  assert.throws(() => findStatusItem([]), /expected one/);
});

test("parses ps RSS and preserves raw output on failure", () => {
  const output =
    "  42  1024 Mon Aug 31 12:10:11 2026\n43 2048 Mon Aug 31 12:10:12 2026\n";
  assert.deepEqual(parsePsRss(output, [42, 43]), [
    { pid: 42, rssBytes: 2 ** 20, startedAt: "Mon Aug 31 12:10:11 2026" },
    { pid: 43, rssBytes: 2 ** 21, startedAt: "Mon Aug 31 12:10:12 2026" },
  ]);
  assert.throws(
    () => parsePsRss("HEADER\n", [42]),
    (error) => error.rawOutput === "HEADER\n",
  );
  assert.throws(
    () => parsePsRss(output, [42]),
    (error) => error.rawOutput === output,
  );
});

test("parses formatted, byte, and native footprint output", () => {
  const formatted = `Process: antiburn [42]\nphys_footprint: 12.5 MiB\nProcess: WebContent [43]\nphys_footprint = 20 MB\nSummary TOTAL physical footprint: 30 MiB\n`;
  const parsed = parseFootprint(formatted, [42, 43]);
  assert.equal(parsed.processes[0].physicalFootprintBytes, 12.5 * 2 ** 20);
  assert.equal(parsed.aggregatePhysicalFootprintBytes, 30 * 2 ** 20);
  const native = `======================================================================\nsleep [42]: 64-bit    Footprint: 884952 B (16384 bytes per page)\n======================================================================\n\nAuxiliary data:\n    phys_footprint: 901336 B\n\n======================================================================\nSummary Footprint: 884952 B\n======================================================================\n`;
  assert.equal(
    parseFootprint(native, [42]).aggregatePhysicalFootprintBytes,
    884952,
  );
  const single = `sleep [42]: 64-bit    Footprint: 1 B\nphys_footprint: 901336 B\n`;
  assert.equal(
    parseFootprint(single, [42]).aggregatePhysicalFootprintBytes,
    901336,
  );
});

test("collects memory and verifies identities after footprint", async () => {
  const calls = [];
  const command = async (file, arguments_) => {
    calls.push([file, arguments_]);
    if (file === "/bin/ps") {
      return {
        stdout:
          "42 100 Mon Aug 31 12:10:11 2026\n43 200 Mon Aug 31 12:10:12 2026\n",
      };
    }
    return {
      stdout:
        "Process: shell [42]\nphys_footprint: 1000 B\nProcess: renderer [43]\nphys_footprint: 2000 B\nTOTAL 2500 B\n",
    };
  };
  const result = await collectMemory(
    [
      { role: "shell", pid: 42, startedAt: "Mon Aug 31 12:10:11 2026" },
      { role: "renderer", pid: 43, startedAt: "Mon Aug 31 12:10:12 2026" },
    ],
    "both",
    { command },
  );
  assert.equal(result.rssSumBytes, 300 * 1024);
  assert.equal(result.aggregatePhysicalFootprintBytes, 2500);
  assert.equal(calls.filter(([file]) => file === "/bin/ps").length, 2);
});

test("calculates statistics and keeps independent run summaries", () => {
  assert.deepEqual(statistics([4, 1, 3, 2]), {
    count: 4,
    minimum: 1,
    median: 2.5,
    p95: 4,
    maximum: 4,
    mean: 2.5,
    standardDeviation: Math.sqrt(1.25),
  });
  const samples = [
    { run: 1, process: { role: "renderer" }, memory: { rssBytes: 10 } },
    { run: 1, process: { role: "renderer" }, memory: { rssBytes: 20 } },
    { run: 2, process: { role: "renderer" }, memory: { rssBytes: 100 } },
  ];
  const summary = summarizeSamples(samples);
  assert.equal(summary.runs.length, 2);
  assert.equal(summary.runs[0].median, 15);
  assert.equal(summary.acrossRuns[0].runMedianSummary.count, 2);
});

test("records only declared popover phases", () => {
  const report = { phaseTimings: [] };
  assert.equal(recordPhase(report, 1, "shell-idle").phase, "shell-idle");
  assert.throws(() => recordPhase(report, 1, "settings-ready"), /unknown/);
});

test("formats the same samples as JSON, NDJSON, CSV, and table", () => {
  const sample = {
    run: 1,
    scenario: "popover",
    phase: "popover-visible-settled",
    sequence: 1,
    process: { role: "renderer", pid: 42 },
    memory: { rssBytes: 1024 },
    dimensions: { accessibilityNodes: 10, syntheticSessionLabels: 1 },
  };
  const report = {
    schemaVersion: 2,
    configuration: {},
    platform: {},
    phaseTimings: [],
    samples: [sample],
    summaries: summarizeSamples([sample]),
    failures: [],
  };
  assert.deepEqual(JSON.parse(formatReport(report, "json")).samples, [sample]);
  assert.match(formatReport(report, "ndjson"), /"type":"sample"/);
  assert.match(formatReport(report, "csv"), /accessibilityNodes/);
  assert.match(formatReport(report, "table"), /1\.0 KiB/);
});
