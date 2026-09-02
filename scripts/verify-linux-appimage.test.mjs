import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import { chmod, mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";

const SCRIPT = join(
  dirname(fileURLToPath(import.meta.url)),
  "verify-linux-appimage.mjs",
);

async function appImageFixture(bundled) {
  const directory = await mkdtemp(join(tmpdir(), "antiburn-appimage-test-"));
  const appImage = join(directory, "antiburn.AppImage");
  await writeFile(
    appImage,
    `#!/bin/sh
set -eu
test "\${1:-}" = --appimage-extract
mkdir -p squashfs-root/usr/lib
${bundled ? "printf bundled > squashfs-root/usr/lib/libwayland-client.so.0" : ":"}
`,
  );
  await chmod(appImage, 0o755);
  return { appImage, directory };
}

test("accepts an AppImage that uses the host Wayland client", async (t) => {
  const subject = await appImageFixture(false);
  t.after(() => rm(subject.directory, { recursive: true, force: true }));

  const output = execFileSync(process.execPath, [SCRIPT, subject.appImage], {
    encoding: "utf8",
  });

  assert.match(output, /uses the host Wayland client library/);
});

test("rejects an AppImage that bundles the Wayland client", async (t) => {
  const subject = await appImageFixture(true);
  t.after(() => rm(subject.directory, { recursive: true, force: true }));

  const result = spawnSync(process.execPath, [SCRIPT, subject.appImage], {
    encoding: "utf8",
  });

  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /bundles usr\/lib\/libwayland-client\.so\.0/);
});
