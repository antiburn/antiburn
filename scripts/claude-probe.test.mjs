import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import test from "node:test";

const root = resolve(import.meta.dirname, "..");
const probe = join(root, "scripts", "dev", "claude-probe.sh");
const nowMs = 1_700_000_000_000;

// Synthetic values. The probe must never print any of them.
const secrets = [
  "synthetic-access-token",
  "synthetic-refresh-token",
  "synthetic-account-id",
  "synthetic-scope",
  "synthetic-mcp-token",
];

function shapeOf(contents) {
  const directory = mkdtempSync(join(tmpdir(), "antiburn-claude-probe-test-"));
  const file = join(directory, "credentials.json");
  writeFileSync(file, contents);
  const result = spawnSync("bash", [probe, "--shape-from-file", file], {
    encoding: "utf8",
    env: { ...process.env, CLAUDE_PROBE_NOW_MS: String(nowMs) },
  });
  assert.equal(result.status, 0, result.stderr);
  for (const secret of secrets) {
    assert.ok(!result.stdout.includes(secret), `output shows ${secret}`);
    assert.ok(!result.stderr.includes(secret), `stderr shows ${secret}`);
  }
  return result.stdout;
}

function login(fields) {
  return JSON.stringify({
    claudeAiOauth: fields,
    mcpOAuth: { "server|synthetic": { accessToken: "synthetic-mcp-token" } },
  });
}

test("a live login shows non-empty keys and a future expiry", () => {
  const output = shapeOf(
    login({
      accessToken: "synthetic-access-token",
      refreshToken: "synthetic-refresh-token",
      expiresAt: nowMs + 90 * 60_000,
      scopes: ["synthetic-scope", "synthetic-scope"],
      accountUuid: "synthetic-account-id",
    }),
  );
  assert.match(output, /- top-level keys: claudeAiOauth, mcpOAuth/);
  assert.match(output, /  - mcpOAuth: object, 1 keys/);
  assert.match(output, /  - accessToken: string, non-empty/);
  assert.match(output, /  - refreshToken: string, non-empty/);
  assert.match(output, /  - expiresAt: number, non-zero/);
  assert.match(output, /  - scopes: array, 2 items/);
  assert.match(output, /- claudeAiOauth.expiresAt: future, in about 90 min/);
  assert.ok(!output.includes("server|synthetic"));
});

test("an expired login shows a past expiry", () => {
  const output = shapeOf(
    login({
      accessToken: "synthetic-access-token",
      refreshToken: "synthetic-refresh-token",
      expiresAt: nowMs - 30 * 60_000,
    }),
  );
  assert.match(output, /- claudeAiOauth.expiresAt: past, about 30 min ago/);
});

test("a blank login shows empty, blank and zero values", () => {
  const output = shapeOf(
    login({
      accessToken: "",
      refreshToken: "  ",
      expiresAt: 0,
      scopes: [],
      subscriptionType: null,
    }),
  );
  assert.match(output, /  - accessToken: string, empty/);
  assert.match(output, /  - refreshToken: string, blank/);
  assert.match(output, /  - expiresAt: number, zero/);
  assert.match(output, /  - scopes: array, 0 items/);
  assert.match(output, /  - subscriptionType: null, null/);
  assert.match(
    output,
    /- claudeAiOauth.expiresAt: not set \(zero or negative\)/,
  );
});

test("an MCP-only item shows that claudeAiOauth is absent", () => {
  const output = shapeOf(
    JSON.stringify({
      mcpOAuth: { synthetic: { accessToken: "synthetic-mcp-token" } },
    }),
  );
  assert.match(output, /- top-level keys: mcpOAuth/);
  assert.match(output, /- claudeAiOauth: absent/);
});

test("invalid JSON shows no content", () => {
  const output = shapeOf(
    '{"claudeAiOauth":{"accessToken":"synthetic-access-token"',
  );
  assert.equal(output, "- data: not valid JSON\n");
});

test("empty data and a non-object show their type only", () => {
  assert.equal(shapeOf(""), "- data: empty\n");
  assert.equal(
    shapeOf('["synthetic-scope"]'),
    "- data: JSON array, not an object\n",
  );
});
