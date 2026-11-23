import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { chmod, mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";

const SCRIPT = join(
  dirname(fileURLToPath(import.meta.url)),
  "verify-linux-appimage.mjs",
);
const TARGET = "x86_64-unknown-linux-gnu";
const CLI_BYTES = "test CLI";
const MANIFEST = {
  schema: 1,
  target: TARGET,
  executable: "antiburn-cli",
  sha256: createHash("sha256").update(CLI_BYTES).digest("hex"),
};

async function appImageFixture(
  t,
  {
    bundled = false,
    cli = CLI_BYTES,
    executable = true,
    manifest = JSON.stringify(MANIFEST),
  } = {},
) {
  const directory = await mkdtemp(join(tmpdir(), "antiburn-appimage-test-"));
  t.after(() => rm(directory, { recursive: true, force: true }));
  const appImage = join(directory, "antiburn.AppImage");
  await writeFile(
    appImage,
    `#!/bin/sh
set -eu
test "\${1:-}" = --appimage-extract
mkdir -p squashfs-root/usr/lib
${bundled ? "printf bundled > squashfs-root/usr/lib/libwayland-client.so.0" : ":"}
cd squashfs-root/usr/lib
mkdir -p antiburn/cli-payload
cd antiburn/cli-payload
${
  manifest === null
    ? ":"
    : `cat > manifest.json <<'MANIFEST'
${manifest}
MANIFEST`
}
${
  cli === null
    ? ":"
    : `printf '%s' '${cli}' > antiburn-cli
chmod ${executable ? "755" : "644"} antiburn-cli`
}
`,
  );
  await chmod(appImage, 0o755);
  return appImage;
}

test("accepts an AppImage with a valid CLI that uses the host Wayland client", async (t) => {
  const subject = await appImageFixture(t);
  const output = execFileSync(process.execPath, [SCRIPT, subject, TARGET], {
    encoding: "utf8",
  });

  assert.match(output, /uses the host Wayland client library/);
  assert.match(
    output,
    /CLI payload matches its manifest for x86_64-unknown-linux-gnu/,
  );
});

for (const [name, fixture, diagnostic] of [
  [
    "a bundled Wayland client",
    { bundled: true },
    /bundles usr\/lib\/libwayland-client\.so\.0/,
  ],
  [
    "altered CLI bytes",
    { cli: "modified" },
    /antiburn\.AppImage: CLI SHA-256 mismatch: expected [0-9a-f]{64}, got [0-9a-f]{64}/,
  ],
  ["a missing CLI", { cli: null }, /ENOENT.*antiburn-cli/],
  ["a non-executable CLI", { executable: false }, /must be an executable file/],
  ["a missing manifest", { manifest: null }, /Cannot read .*manifest\.json/],
  ["a malformed manifest", { manifest: "{" }, /Cannot read .*manifest\.json/],
  [
    "a wrong target",
    {
      manifest: JSON.stringify({
        ...MANIFEST,
        target: "aarch64-unknown-linux-gnu",
      }),
    },
    /Invalid .*manifest\.json for x86_64/,
  ],
  [
    "an invalid hash",
    { manifest: JSON.stringify({ ...MANIFEST, sha256: "123" }) },
    /Invalid .*manifest\.json/,
  ],
]) {
  test(`rejects an AppImage with ${name}`, async (t) => {
    const subject = await appImageFixture(t, fixture);
    const result = spawnSync(process.execPath, [SCRIPT, subject, TARGET], {
      encoding: "utf8",
    });

    assert.equal(result.status, 1);
    assert.match(result.stderr, diagnostic);
  });
}
