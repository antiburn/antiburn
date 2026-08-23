import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import { chmodSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { after, before, test } from "node:test";
import { fileURLToPath } from "node:url";

import { AISLOP_BIN, patchFiles, run } from "./codex-aislop-hook.mjs";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const probeRoot = join(root, ".aislop", `probe-${process.pid}`);
const fx = join(probeRoot, "fx");
const shadow = join(probeRoot, "shadow");
const probeRelative = `.aislop/probe-${process.pid}/fx/probe.rs`;
const cleanRelative = `.aislop/probe-${process.pid}/fx/clean.rs`;
const quotedRelative = `.aislop/probe-${process.pid}/fx/a "q"\\b.rs`;
const probeSource = 'const API_URL: &str = "https://api.antiburn.invalid";\n';
const telemetryEnv = {
  ...process.env,
  AISLOP_TELEMETRY_DEBUG: "1",
  AISLOP_TELEMETRY_DRY_RUN: "1",
};

function executable(path, text) {
  writeFileSync(path, text);
  chmodSync(path, 0o755);
}

function eventFor(path) {
  return JSON.stringify({
    tool_input: {
      command: `*** Begin Patch\n*** Update File: ${path}\n@@\n-old\n+new\n*** End Patch`,
    },
  });
}

function runCommand(command, input, env = telemetryEnv, cwd = root) {
  return spawnSync("/bin/sh", ["-c", command], {
    cwd,
    env,
    input,
    encoding: "utf8",
  });
}

function assertProbeResult(result, path = probeRelative) {
  assert.equal(result.status, 0, result.stderr);
  const output = JSON.parse(result.stdout);
  assert.equal("decision" in output, false);
  const feedback = JSON.parse(output.hookSpecificOutput.additionalContext);
  assert.equal(feedback.findings.length, 1);
  assert.deepEqual(
    {
      ruleId: feedback.findings[0].ruleId,
      severity: feedback.findings[0].severity,
      file: feedback.findings[0].file,
    },
    { ruleId: "ai-slop/hardcoded-url", severity: "error", file: path },
  );
  assert.doesNotMatch(result.stderr, /\[telemetry\]/);
}

async function captureRun(input, bin) {
  let stderr = "";
  const originalWrite = process.stderr.write;
  process.stderr.write = (chunk) => {
    stderr += String(chunk);
    return true;
  };
  try {
    return { code: await run(input, bin), stderr };
  } finally {
    process.stderr.write = originalWrite;
  }
}

function assertOneLine(text) {
  assert.match(text, /^[^\n]+\n$/);
}

before(() => {
  mkdirSync(fx, { recursive: true });
  mkdirSync(shadow, { recursive: true });
  writeFileSync(join(fx, "probe.rs"), probeSource);
  writeFileSync(join(fx, "clean.rs"), "fn main() {}\n");
  writeFileSync(join(fx, 'a "q"\\b.rs'), probeSource);
  executable(join(fx, "fail-bin"), "#!/bin/sh\nexit 3\n");
  executable(join(fx, "signal-bin"), "#!/bin/sh\nkill -TERM $$\n");
  executable(join(shadow, "aislop"), "#!/bin/sh\necho shadow-aislop\n");
});

after(() => {
  rmSync(probeRoot, { recursive: true, force: true });
});

test("the hook uses the pinned aislop executable", () => {
  const packageJson = JSON.parse(readFileSync(join(root, "package.json"), "utf8"));
  const version = packageJson.devDependencies.aislop;
  assert.equal(version, "0.14.0");
  assert.equal(execFileSync(AISLOP_BIN, ["--version"], { encoding: "utf8" }).trim(), version);
  assert.match(AISLOP_BIN, /node_modules\/.bin\/aislop$/);
});

test("the committed hook files have the required shapes", () => {
  const claude = JSON.parse(readFileSync(join(root, ".claude/settings.json"), "utf8"));
  assert.deepEqual(Object.keys(claude.hooks), ["PostToolUse"]);
  assert.equal(claude.hooks.PostToolUse.length, 1);
  const claudeGroup = claude.hooks.PostToolUse[0];
  assert.equal(claudeGroup.matcher, "Edit|Write|MultiEdit");
  assert.equal(claudeGroup.hooks.length, 1);
  const claudeCommand = claudeGroup.hooks[0].command;
  assert.match(claudeCommand, /AISLOP_NO_TELEMETRY=1/);
  assert.match(claudeCommand, /node_modules\/.bin\/aislop/);
  const serialized = JSON.stringify(claude);
  assert.doesNotMatch(serialized, /"__aislop":(?!null)/);
  assert.ok(
    ![
      "aislop hook claude",
      "aislop hook claude --on-file-changed",
      "aislop hook claude --stop",
    ].includes(claudeCommand),
  );

  const codex = JSON.parse(readFileSync(join(root, ".codex/hooks.json"), "utf8"));
  assert.equal(typeof codex.description, "string");
  assert.deepEqual(Object.keys(codex.hooks), ["PostToolUse"]);
  assert.equal(codex.hooks.PostToolUse.length, 1);
  const codexGroup = codex.hooks.PostToolUse[0];
  assert.equal(codexGroup.matcher, "^apply_patch$");
  assert.equal(codexGroup.hooks.length, 1);
  assert.equal(codexGroup.hooks[0].type, "command");
  assert.match(codexGroup.hooks[0].command, /scripts\/codex-aislop-hook\.mjs/);
  assert.equal(typeof codexGroup.hooks[0].timeout, "number");
});

test("the parser follows the apply_patch structure", () => {
  assert.deepEqual(
    patchFiles(`*** Begin Patch\n*** Update File: one.rs\n@@\n-old\n+new\n@@\n-a\n+b\n*** Add File: two.rs\n+first\n@@\n+second\n*** Update File: one.rs\n@@\n-x\n+y\n*** End Patch`),
    ["one.rs", "two.rs"],
  );
  assert.deepEqual(
    patchFiles("*** Begin Patch\n*** Delete File: gone.rs\n*** End Patch"),
    [],
  );
  assert.deepEqual(
    patchFiles("*** Begin Patch\n*** Update File: old.rs\n*** Move to: new.rs\n@@\n-old\n+new\n*** End Patch"),
    ["new.rs"],
  );
  const special = 'space name-λ-"-\\.rs';
  assert.deepEqual(patchFiles(`*** Add File: ${special}\n+x`), [special]);
  assert.deepEqual(patchFiles(""), []);
  assert.deepEqual(patchFiles("ordinary prose"), []);
  assert.deepEqual(patchFiles("  *** Update File: padded.rs  \n@@\n x"), ["padded.rs"]);
  assert.deepEqual(patchFiles("\t*** Update File: tabbed.rs\t\n@@\n x"), ["tabbed.rs"]);
  assert.deepEqual(patchFiles("*** Update File: crlf.rs\r\n@@\r\n x\r"), ["crlf.rs"]);
  assert.deepEqual(patchFiles("*** Update File:   spaced.rs\n@@\n x"), ["  spaced.rs"]);
  assert.deepEqual(patchFiles("*** Add File: added.rs\n+*** Add File: marker.rs"), ["added.rs"]);
  assert.deepEqual(
    patchFiles("*** Update File: updated.rs\n@@\n *** Add File: context.rs"),
    ["updated.rs"],
  );
  assert.deepEqual(
    patchFiles("*** Add File: first.rs\n  *** Add File: second.rs\n+content"),
    ["first.rs", "second.rs"],
  );
});

test("the Claude hook reports one finding without telemetry", () => {
  const claude = JSON.parse(readFileSync(join(root, ".claude/settings.json"), "utf8"));
  const command = claude.hooks.PostToolUse[0].hooks[0].command;
  const env = { ...telemetryEnv, CLAUDE_PROJECT_DIR: root };
  const input = JSON.stringify({ tool_input: { edits: [{ file_path: probeRelative }] } });
  assertProbeResult(runCommand(command, input, env));
});

test("the Codex hook preserves paths and reports without telemetry", () => {
  const codex = JSON.parse(readFileSync(join(root, ".codex/hooks.json"), "utf8"));
  const command = codex.hooks.PostToolUse[0].hooks[0].command;
  assertProbeResult(runCommand(command, eventFor(probeRelative)));

  const clean = runCommand(command, eventFor(cleanRelative));
  assert.equal(clean.status, 0, clean.stderr);
  const cleanEnvelope = JSON.parse(clean.stdout);
  assert.deepEqual(Object.keys(cleanEnvelope), ["hookSpecificOutput"]);
  assert.equal(cleanEnvelope.hookSpecificOutput.hookEventName, "PostToolUse");
  const cleanFeedback = JSON.parse(cleanEnvelope.hookSpecificOutput.additionalContext);
  assert.equal(cleanFeedback.schema, "aislop.hook.v2");
  assert.deepEqual(cleanFeedback.counts, { error: 0, warning: 0, fixable: 0, total: 0 });
  assert.deepEqual(cleanFeedback.findings, []);
  assert.doesNotMatch(clean.stderr, /\[telemetry\]/);

  assertProbeResult(runCommand(command, eventFor(quotedRelative)), quotedRelative);
});

test("the adapter applies its silent and fault exit contract", async () => {
  const failBin = join(fx, "fail-bin");
  const signalBin = join(fx, "signal-bin");
  const silentInputs = [
    "",
    "  \n\t",
    JSON.stringify({ tool_input: {} }),
    JSON.stringify({ tool_input: { command: 7 } }),
    JSON.stringify({
      tool_input: { command: "*** Begin Patch\n*** Delete File: gone.rs\n*** End Patch" },
    }),
  ];
  for (const input of silentInputs) {
    assert.deepEqual(await captureRun(input, failBin), { code: 0, stderr: "" });
  }

  for (const input of ["123", "not JSON"]) {
    const result = await captureRun(input, failBin);
    assert.equal(result.code, 0);
    assertOneLine(result.stderr);
  }

  for (const bin of [join(fx, "missing-bin"), failBin, signalBin]) {
    const result = await captureRun(eventFor(probeRelative), bin);
    assert.equal(result.code, 1);
    assertOneLine(result.stderr);
  }
});

test("the Codex command propagates repository setup failure", () => {
  const codex = JSON.parse(readFileSync(join(root, ".codex/hooks.json"), "utf8"));
  const command = codex.hooks.PostToolUse[0].hooks[0].command;
  const outside = mkdtempSync(join(tmpdir(), "codex-aislop-hook-"));
  try {
    const result = runCommand(command, eventFor(probeRelative), telemetryEnv, outside);
    assert.notEqual(result.status, 0);
  } finally {
    rmSync(outside, { recursive: true, force: true });
  }
});

test("the absolute pin ignores an aislop shadow on PATH", () => {
  const env = { ...telemetryEnv, PATH: `${shadow}:${process.env.PATH}` };
  assert.equal(
    execFileSync("/bin/sh", ["-c", "command -v aislop"], { env, encoding: "utf8" }).trim(),
    join(shadow, "aislop"),
  );
  assert.equal(execFileSync(AISLOP_BIN, ["--version"], { env, encoding: "utf8" }).trim(), "0.14.0");
  const codex = JSON.parse(readFileSync(join(root, ".codex/hooks.json"), "utf8"));
  const command = codex.hooks.PostToolUse[0].hooks[0].command;
  assertProbeResult(runCommand(command, eventFor(probeRelative), env));
});
