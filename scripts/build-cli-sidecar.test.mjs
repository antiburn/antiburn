import { test } from "node:test";
import assert from "node:assert/strict";
import {
  artifactPaths,
  targets,
  validateArchitecture,
  validateNativeDependencies,
} from "./build-cli-sidecar.mjs";

test("build uses the public CLI artifact and stages the private payload name", () => {
  assert.match(
    artifactPaths("x86_64-unknown-linux-gnu").source,
    /apps\/cli\/target\/sidecar-build\/x86_64-unknown-linux-gnu\/release\/antiburn$/,
  );
  const paths = artifactPaths("x86_64-unknown-linux-gnu", "/isolated/target");
  assert.equal(paths.source, "/isolated/target/x86_64-unknown-linux-gnu/release/antiburn");
  assert.match(paths.destination, /cli-payload\/antiburn-cli$/);
  const windows = artifactPaths("aarch64-pc-windows-msvc", "/isolated/target");
  assert.equal(windows.source, "/isolated/target/aarch64-pc-windows-msvc/release/antiburn.exe");
  assert.match(windows.destination, /cli-payload[\\/]antiburn-cli\.exe$/);
});

test("CLI architecture rejects mislabeled binaries for all six release targets", () => {
  for (const target of targets) {
    const b = Buffer.alloc(256);
    const arm = target.startsWith("aarch64");
    if (target.includes("apple")) {
      b.writeUInt32LE(0xfeedfacf);
      b.writeUInt32LE(arm ? 0x100000c : 0x1000007, 4);
    } else if (target.includes("windows")) {
      b.write("MZ");
      b.writeUInt32LE(64, 0x3c);
      b.writeUInt32LE(0x4550, 64);
      b.writeUInt16LE(arm ? 0xaa64 : 0x8664, 68);
    } else {
      b.write("7f454c46", "hex");
      b[4] = 2;
      b[5] = 1;
      b.writeUInt16LE(arm ? 183 : 62, 18);
    }
    validateArchitecture(b, target);
    assert.throws(() =>
      validateArchitecture(
        b,
        target.replace(arm ? "aarch64" : "x86_64", arm ? "x86_64" : "aarch64"),
      ),
    );
  }
  assert.throws(() => validateArchitecture(Buffer.alloc(256), "unknown"));
});

test("native dependency checks reject unbundled runtime libraries", () => {
  assert.deepEqual(
    validateNativeDependencies(
      "binary:\n /usr/lib/libSystem.B.dylib (compatibility version 1.0.0)\n",
      "aarch64-apple-darwin",
    ),
    ["/usr/lib/libSystem.B.dylib"],
  );
  assert.throws(() =>
    validateNativeDependencies(
      "binary:\n /opt/local/lib/private.dylib (version 1)\n",
      "aarch64-apple-darwin",
    ),
  );
  assert.deepEqual(
    validateNativeDependencies(
      " 0x1 (NEEDED) Shared library: [libc.so.6]",
      "x86_64-unknown-linux-gnu",
    ),
    ["libc.so.6"],
  );
  assert.throws(() =>
    validateNativeDependencies(
      " 0x1 (NEEDED) Shared library: [libssl.so.3]",
      "x86_64-unknown-linux-gnu",
    ),
  );
  assert.deepEqual(
    validateNativeDependencies(
      "    KERNEL32.dll\r\n    api-ms-win-core-synch-l1-2-0.dll\r\n",
      "aarch64-pc-windows-msvc",
    ),
    ["kernel32.dll", "api-ms-win-core-synch-l1-2-0.dll"],
  );
  assert.throws(() =>
    validateNativeDependencies(
      "    VCRUNTIME140.dll\r\n",
      "x86_64-pc-windows-msvc",
    ),
  );
});
