import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { copyFileSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
export const targets = [
  "aarch64-apple-darwin",
  "x86_64-apple-darwin",
  "aarch64-pc-windows-msvc",
  "x86_64-pc-windows-msvc",
  "aarch64-unknown-linux-gnu",
  "x86_64-unknown-linux-gnu",
];

export function validateArchitecture(bytes, target) {
  if (!targets.includes(target))
    throw new Error(`Unsupported CLI target: ${target}`);
  const arm = target.startsWith("aarch64");
  let valid = false;
  if (target.includes("apple")) {
    valid =
      bytes.readUInt32LE(0) === 0xfeedfacf &&
      bytes.readUInt32LE(4) === (arm ? 0x100000c : 0x1000007);
  } else if (target.includes("windows")) {
    const pe = bytes.readUInt32LE(0x3c);
    valid =
      bytes.toString("ascii", 0, 2) === "MZ" &&
      bytes.readUInt32LE(pe) === 0x4550 &&
      bytes.readUInt16LE(pe + 4) === (arm ? 0xaa64 : 0x8664);
  } else {
    valid =
      bytes.toString("hex", 0, 4) === "7f454c46" &&
      bytes[4] === 2 &&
      bytes[5] === 1 &&
      bytes.readUInt16LE(18) === (arm ? 183 : 62);
  }
  if (!valid) throw new Error(`CLI architecture does not match ${target}`);
}

export function build(target) {
  if (!targets.includes(target))
    throw new Error("Pass one supported --target triple");
  const executable = target.includes("windows")
    ? "antiburn-cli.exe"
    : "antiburn-cli";
  const manifest = join(root, "apps/cli/Cargo.toml");
  const env = { ...process.env };
  if (target.includes("windows")) {
    if (env.CARGO_ENCODED_RUSTFLAGS)
      env.CARGO_ENCODED_RUSTFLAGS += "\x1f-C\x1ftarget-feature=+crt-static";
    else env.RUSTFLAGS = `${env.RUSTFLAGS || ""} -C target-feature=+crt-static`;
  }
  execFileSync(
    "cargo",
    [
      "build",
      "--locked",
      "--release",
      "--manifest-path",
      manifest,
      "--target",
      target,
      "--bin",
      "antiburn-cli",
    ],
    { stdio: "inherit", env },
  );
  const source = join(
    process.env.CARGO_TARGET_DIR || join(root, "apps/cli/target"),
    target,
    "release",
    executable,
  );
  const destination = join(root, "apps/desktop/src-tauri/cli-payload");
  mkdirSync(destination, { recursive: true });
  copyFileSync(source, join(destination, executable));
  auditDependencies(join(destination, executable), target, destination);
  finalize(target);
}

export function validateNativeDependencies(output, target) {
  let dependencies;
  if (target.includes("windows")) {
    dependencies = [...output.matchAll(/^\s+([a-z0-9_.-]+\.dll)\s*$/gim)].map(
      (m) => m[1].toLowerCase(),
    );
    const system =
      /^(?:kernel32|ntdll|userenv|ws2_32|bcrypt|bcryptprimitives|advapi32|crypt32|secur32|shell32|ole32|oleaut32|user32|gdi32|msvcrt|normaliz|iphlpapi|ncrypt|rpcrt4|version|winmm|psapi|dbghelp|ucrtbase)\.dll$|^api-ms-win-[a-z0-9-]+\.dll$/;
    if (dependencies.some((name) => !system.test(name)))
      throw new Error("CLI has a non-system Windows dependency");
  } else if (target.includes("apple")) {
    dependencies = output
      .split("\n")
      .slice(1)
      .map((line) => line.trim().split(" ")[0])
      .filter(Boolean);
    if (
      dependencies.some(
        (name) =>
          !name.startsWith("/usr/lib/") && !name.startsWith("/System/Library/"),
      )
    )
      throw new Error("CLI has an unbundled macOS dependency");
  } else {
    dependencies = [...output.matchAll(/\(NEEDED\).*\[([^\]]+)\]/g)].map(
      (m) => m[1],
    );
    if (
      dependencies.some(
        (name) =>
          !/^(?:libc|libm|libgcc_s|libpthread|libdl|librt|libresolv)\.so(?:\.\d+)*$|^ld-linux-[a-z0-9_-]+\.so(?:\.\d+)*$/.test(
            name,
          ),
      )
    )
      throw new Error("CLI has an unbundled Linux dependency");
  }
  if (!dependencies.length)
    throw new Error(
      "Native CLI dependency inspection produced no dependencies",
    );
  return dependencies;
}

function auditDependencies(binary, target, destination) {
  let output;
  if (target.includes("windows")) {
    output = execFileSync(
      "powershell.exe",
      [
        "-NoProfile",
        "-File",
        join(root, "scripts/cli-native-dependencies.ps1"),
        "-Binary",
        binary,
      ],
      { encoding: "utf8" },
    );
  } else if (target.includes("apple")) {
    output = execFileSync("otool", ["-L", binary], { encoding: "utf8" });
  } else {
    output = execFileSync("readelf", ["-d", binary], { encoding: "utf8" });
  }
  writeFileSync(
    join(destination, "native-dependencies.json"),
    JSON.stringify(
      { target, dependencies: validateNativeDependencies(output, target) },
      null,
      2,
    ) + "\n",
  );
}

export function finalize(target) {
  if (!targets.includes(target))
    throw new Error("Pass one supported --target triple");
  const executable = target.includes("windows")
    ? "antiburn-cli.exe"
    : "antiburn-cli";
  const directory = join(root, "apps/desktop/src-tauri/cli-payload");
  const bytes = readFileSync(join(directory, executable));
  validateArchitecture(bytes, target);
  const version = /^version = "([^"]+)"/m.exec(
    readFileSync(join(root, "apps/cli/Cargo.toml"), "utf8"),
  )[1];
  writeFileSync(
    join(directory, "manifest.json"),
    JSON.stringify(
      {
        schema: 1,
        target,
        executable,
        version,
        sha256: createHash("sha256").update(bytes).digest("hex"),
        delivery_protocols: ["delivery-v1"],
      },
      null,
      2,
    ) + "\n",
  );
}

if (
  process.argv[1] &&
  resolve(process.argv[1]) === fileURLToPath(import.meta.url)
) {
  const target = process.argv[process.argv.indexOf("--target") + 1];
  if (process.argv.includes("--finalize")) finalize(target);
  else build(target);
}
