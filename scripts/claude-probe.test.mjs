import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import {
  chmodSync,
  mkdirSync,
  mkdtempSync,
  readdirSync,
  readFileSync,
  writeFileSync,
} from "node:fs";
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

function hex(text) {
  return Buffer.from(text, "utf8").toString("hex");
}

test("hex-encoded data is decoded and shows its shape", () => {
  const output = shapeOf(
    `${hex(
      login({
        accessToken: "synthetic-access-token",
        refreshToken: "",
        expiresAt: nowMs + 5 * 60_000,
      }),
    )}\n`,
  );
  assert.match(output, /^- data: hex-encoded, decoded\n/);
  assert.match(output, /  - accessToken: string, non-empty/);
  assert.match(output, /  - refreshToken: string, empty/);
  assert.match(output, /- claudeAiOauth.expiresAt: future, in about 5 min/);
});

test("hex that does not decode to JSON shows no content", () => {
  assert.equal(
    shapeOf(hex('{"synthetic-access-token"')),
    "- data: not valid JSON (looks hex-encoded)\n",
  );
  assert.equal(shapeOf("abc"), "- data: not valid JSON (looks hex-encoded)\n");
});

// A stub `security` for the full probe. The plain item returns hex, as
// `security -w` does on some macOS versions. The hashed item fails the
// secret read with exit 51, as a denied Keychain prompt does.
const securityStub = `#!/bin/sh
case "$1" in
  dump-keychain)
    echo '    "svce"<blob>="Claude Code-credentials"'
    echo '    "svce"<blob>="Claude Code-credentials-0abc"'
    exit 0 ;;
esac
svc=""; read=0
while [ $# -gt 0 ]; do
  case "$1" in -s) svc="$2"; shift ;; -w) read=1 ;; esac
  shift
done
case "$svc" in
  "Claude Code-credentials"|"Claude Code-credentials-0abc") ;;
  *) exit 44 ;;
esac
if [ "$read" -eq 0 ]; then
  echo '    "acct"<blob>="synthetic-account-id"'
  printf '%s\\n' '    "mdat"<timedate>=0x00  "20240101000000Z\\000"'
  exit 0
fi
[ "$svc" = "Claude Code-credentials" ] || exit 51
printf '%s\\n' "$PROBE_STUB_HEX"
`;

test(
  "the full probe decodes hex Keychain output and reports a denied read",
  {
    skip: process.platform !== "darwin" && "the full probe runs on macOS only",
  },
  () => {
    const home = mkdtempSync(join(tmpdir(), "antiburn-claude-probe-home-"));
    const bin = join(home, "bin");
    mkdirSync(bin);
    writeFileSync(join(bin, "security"), securityStub);
    // Keep the run fast and independent of the host's brew and npm.
    writeFileSync(join(bin, "brew"), "#!/bin/sh\nexit 0\n");
    writeFileSync(join(bin, "npm"), "#!/bin/sh\nexit 0\n");
    for (const name of ["security", "brew", "npm"]) {
      chmodSync(join(bin, name), 0o755);
    }
    const out = join(home, "out");
    const result = spawnSync(
      "bash",
      [probe, "--label", "stub", "--out", out, "--keychain-shape"],
      {
        cwd: home,
        encoding: "utf8",
        env: {
          ...process.env,
          HOME: home,
          PATH: `${bin}:${process.env.PATH}`,
          CLAUDE_CONFIG_DIR: join(home, "no-config"),
          CLAUDE_PROBE_NOW_MS: String(nowMs),
          PROBE_STUB_HEX: hex(
            login({
              accessToken: "synthetic-access-token",
              refreshToken: "synthetic-refresh-token",
              expiresAt: nowMs - 60 * 60_000,
            }),
          ),
        },
      },
    );
    assert.equal(result.status, 0, result.stderr);
    const [file] = readdirSync(out);
    const report = readFileSync(join(out, file), "utf8");
    for (const secret of secrets) {
      assert.ok(!report.includes(secret), `report shows ${secret}`);
    }
    const plain = report.split("- `Claude Code-credentials`: present")[1];
    const hashed = report.split("- `Claude Code-credentials-0abc`: present")[1];
    assert.ok(plain && hashed, report);
    assert.match(
      plain.split("- `Claude Code-credentials-0abc`")[0],
      /    - data: hex-encoded, decoded\n[\s\S]*    - claudeAiOauth.expiresAt: past, about 60 min ago/,
    );
    assert.match(
      hashed,
      /^ \(acct set: yes, mdat: 20240101000000Z\)\n  - login shape \(secret read, can show a Keychain prompt\):\n    - data: unreadable \(security exit 51: /,
    );
    assert.ok(
      !hashed.split("\n- `Claude Safe Storage`")[0].includes("data: empty"),
    );
    assert.match(report, /- `Claude Safe Storage`: absent/);
  },
);
