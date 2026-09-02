#!/usr/bin/env node

import { mkdtemp, rm, stat } from "node:fs/promises";
import { spawnSync } from "node:child_process";
import { tmpdir } from "node:os";
import { basename, join, resolve } from "node:path";
import process from "node:process";

const HOST_LIBRARY = "usr/lib/libwayland-client.so.0";

async function exists(path) {
  try {
    await stat(path);
    return true;
  } catch (error) {
    if (error?.code === "ENOENT") return false;
    throw error;
  }
}

async function main() {
  const [input] = process.argv.slice(2);
  if (!input) throw new Error("Usage: verify-linux-appimage.mjs <AppImage>");

  const appImage = resolve(input);
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
        `${basename(appImage)} extraction failed with status ${result.status ?? "unknown"}`,
      );
    }
    if (await exists(join(work, "squashfs-root", HOST_LIBRARY))) {
      throw new Error(`${basename(appImage)} bundles ${HOST_LIBRARY}`);
    }
    console.log(`${basename(appImage)} uses the host Wayland client library`);
  } finally {
    await rm(work, { recursive: true, force: true });
  }
}

main().catch((error) => {
  console.error(error instanceof Error ? error.message : String(error));
  process.exitCode = 1;
});
