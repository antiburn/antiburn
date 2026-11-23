#!/usr/bin/env node

import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { lstat, mkdtemp, readFile, rm, stat } from "node:fs/promises";
import { tmpdir } from "node:os";
import { basename, join, resolve } from "node:path";
import process from "node:process";

const HOST_LIBRARY = "usr/lib/libwayland-client.so.0";
const CLI_PAYLOAD = "usr/lib/antiburn/cli-payload";
const CLI_EXECUTABLE = "antiburn-cli";

async function exists(path) {
  try {
    await stat(path);
    return true;
  } catch (error) {
    if (error?.code === "ENOENT") return false;
    throw error;
  }
}

/** Verify the extracted CLI bytes against the manifest that desktop reads. */
async function verifyCliPayload(appDir, target) {
  const payload = join(appDir, CLI_PAYLOAD);
  const manifestPath = join(payload, "manifest.json");
  let manifest;
  try {
    manifest = JSON.parse(await readFile(manifestPath, "utf8"));
  } catch (error) {
    throw new Error(
      `Cannot read ${CLI_PAYLOAD}/manifest.json: ${error.message}`,
    );
  }
  if (
    manifest?.schema !== 1 ||
    manifest.target !== target ||
    manifest.executable !== CLI_EXECUTABLE ||
    typeof manifest.sha256 !== "string" ||
    !/^[0-9a-f]{64}$/.test(manifest.sha256)
  ) {
    throw new Error(`Invalid ${CLI_PAYLOAD}/manifest.json for ${target}`);
  }

  const executable = join(payload, CLI_EXECUTABLE);
  const metadata = await lstat(executable);
  if (!metadata.isFile() || (metadata.mode & 0o111) === 0) {
    throw new Error(
      `${CLI_PAYLOAD}/${CLI_EXECUTABLE} must be an executable file`,
    );
  }
  const actual = createHash("sha256")
    .update(await readFile(executable))
    .digest("hex");
  if (actual !== manifest.sha256) {
    throw new Error(
      `CLI SHA-256 mismatch: expected ${manifest.sha256}, got ${actual}`,
    );
  }
}

async function main() {
  const [input, target] = process.argv.slice(2);
  if (!input || !/^(x86_64|aarch64)-unknown-linux-gnu$/.test(target)) {
    throw new Error(
      "Usage: verify-linux-appimage.mjs <AppImage> <Linux target>",
    );
  }

  const appImage = resolve(input);
  const artifact = basename(appImage);
  await stat(appImage);
  const work = await mkdtemp(join(tmpdir(), "antiburn-appimage-check-"));
  try {
    const result = spawnSync(appImage, ["--appimage-extract"], {
      cwd: work,
      env: { ...process.env, APPIMAGE_EXTRACT_AND_RUN: "1" },
      stdio: "ignore",
    });
    if (result.error) throw result.error;
    if (result.status !== 0) {
      throw new Error(
        `${artifact} extraction failed with status ${result.status ?? "unknown"}`,
      );
    }
    const appDir = join(work, "squashfs-root");
    if (await exists(join(appDir, HOST_LIBRARY))) {
      throw new Error(`${artifact} bundles ${HOST_LIBRARY}`);
    }
    try {
      await verifyCliPayload(appDir, target);
    } catch (error) {
      throw new Error(`${artifact}: ${error.message}`);
    }
    console.log(`${artifact} uses the host Wayland client library`);
    console.log(`${artifact} CLI payload matches its manifest for ${target}`);
  } finally {
    await rm(work, { recursive: true, force: true });
  }
}

main().catch((error) => {
  console.error(error instanceof Error ? error.message : String(error));
  process.exitCode = 1;
});
