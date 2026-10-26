import { test } from "node:test";
import assert from "node:assert/strict";
import { targets, validateArchitecture } from "./build-cli-sidecar.mjs";

test("CLI architecture rejects mislabeled binaries for all six release targets", () => {
  for (const target of targets) {
    const b = Buffer.alloc(256);
    const arm = target.startsWith("aarch64");
    if (target.includes("apple")) {
      b.writeUInt32LE(0xfeedfacf); b.writeUInt32LE(arm ? 0x100000c : 0x1000007, 4);
    } else if (target.includes("windows")) {
      b.write("MZ"); b.writeUInt32LE(64, 0x3c); b.writeUInt32LE(0x4550, 64);
      b.writeUInt16LE(arm ? 0xaa64 : 0x8664, 68);
    } else {
      b.write("7f454c46", "hex"); b[4] = 2; b[5] = 1; b.writeUInt16LE(arm ? 183 : 62, 18);
    }
    validateArchitecture(b, target);
    assert.throws(() => validateArchitecture(b, target.replace(arm ? "aarch64" : "x86_64", arm ? "x86_64" : "aarch64")));
  }
  assert.throws(() => validateArchitecture(Buffer.alloc(256), "unknown"));
});
