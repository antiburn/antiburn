import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { copyFileSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
export const targets = [
  "aarch64-apple-darwin", "x86_64-apple-darwin",
  "aarch64-pc-windows-msvc", "x86_64-pc-windows-msvc",
  "aarch64-unknown-linux-gnu", "x86_64-unknown-linux-gnu",
];

export function validateArchitecture(bytes, target) {
  if (!targets.includes(target)) throw new Error(`Unsupported CLI target: ${target}`);
  const arm = target.startsWith("aarch64");
  let valid = false;
  if (target.includes("apple")) {
    valid = bytes.readUInt32LE(0) === 0xfeedfacf &&
      bytes.readUInt32LE(4) === (arm ? 0x100000c : 0x1000007);
  } else if (target.includes("windows")) {
    const pe = bytes.readUInt32LE(0x3c);
    valid = bytes.toString("ascii", 0, 2) === "MZ" &&
      bytes.readUInt32LE(pe) === 0x4550 &&
      bytes.readUInt16LE(pe + 4) === (arm ? 0xaa64 : 0x8664);
  } else {
    valid = bytes.toString("hex", 0, 4) === "7f454c46" && bytes[4] === 2 &&
      bytes[5] === 1 && bytes.readUInt16LE(18) === (arm ? 183 : 62);
  }
  if (!valid) throw new Error(`CLI architecture does not match ${target}`);
}

export function build(target) {
  if (!targets.includes(target)) throw new Error("Pass one supported --target triple");
  const executable = target.includes("windows") ? "antiburn-cli.exe" : "antiburn-cli";
  const manifest = join(root, "apps/cli/Cargo.toml");
  execFileSync("cargo", ["build", "--locked", "--release", "--manifest-path", manifest,
    "--target", target, "--bin", "antiburn-cli"], { stdio: "inherit" });
  const source = join(process.env.CARGO_TARGET_DIR || join(root, "apps/cli/target"), target, "release", executable);
  const destination = join(root, "apps/desktop/src-tauri/cli-payload");
  mkdirSync(destination, { recursive: true });
  copyFileSync(source, join(destination, executable));
  finalize(target);
}

export function finalize(target) {
  if (!targets.includes(target)) throw new Error("Pass one supported --target triple");
  const executable = target.includes("windows") ? "antiburn-cli.exe" : "antiburn-cli";
  const directory = join(root, "apps/desktop/src-tauri/cli-payload");
  const bytes = readFileSync(join(directory, executable));
  validateArchitecture(bytes, target);
  const version = /^version = "([^"]+)"/m.exec(readFileSync(join(root, "apps/cli/Cargo.toml"), "utf8"))[1];
  writeFileSync(join(directory, "manifest.json"), JSON.stringify({
    schema: 1, target, executable, version,
    sha256: createHash("sha256").update(bytes).digest("hex"),
    delivery_protocols: ["delivery-v1"],
  }, null, 2) + "\n");
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const target = process.argv[process.argv.indexOf("--target") + 1];
  if (process.argv.includes("--finalize")) finalize(target);
  else build(target);
}
