import assert from "node:assert/strict";
import {
  mkdtempSync,
  mkdirSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import test from "node:test";
import {
  prepareAppVersion,
  validateAppVersion,
} from "./prepare-app-version.mjs";

function fixture() {
  const root = mkdtempSync(join(tmpdir(), "app-version-"));
  const files = {
    "apps/desktop/package.json":
      '{"version":"0.9.0","dependencies":{"synthetic":"0.9.0"}}\n',
    "apps/desktop/src-tauri/tauri.conf.json":
      '{"version":"0.9.0","identifier":"synthetic.app"}\n',
    "apps/desktop/src-tauri/Cargo.toml":
      '[package]\nname = "antiburn"\nversion = "0.9.0"\n\n[dependencies]\nsynthetic = "0.9.0"\n',
    "apps/desktop/src-tauri/Cargo.lock":
      'version = 4\n\n[[package]]\nname = "antiburn"\nversion = "0.9.0"\n\n[[package]]\nname = "antiburn-remote"\nversion = "0.9.0"\n\n[[package]]\nname = "synthetic"\nversion = "0.9.0"\nchecksum = "synthetic"\n',
    "crates/antiburn-remote/Cargo.toml":
      '[package]\nname = "antiburn-remote"\nversion = "0.9.0"\n\n[dependencies]\nsynthetic = "0.9.0"\n',
    "crates/antiburn-remote/Cargo.lock":
      'version = 4\n\n[[package]]\nname = "antiburn-remote"\nversion = "0.9.0"\n\n[[package]]\nname = "synthetic"\nversion = "0.9.0"\n',
  };
  for (const [file, content] of Object.entries(files)) {
    mkdirSync(dirname(join(root, file)), { recursive: true });
    writeFileSync(join(root, file), content);
  }
  return {
    root,
    files,
    read: (file) => readFileSync(join(root, file), "utf8"),
  };
}

test("changes application and helper versions while preserving dependency versions", () => {
  const data = fixture();
  try {
    prepareAppVersion(data.root, "0.10.0-rc.1");
    for (const file of Object.keys(data.files)) {
      assert.match(data.read(file), /0\.10\.0-rc\.1/);
    }
    assert.equal(
      JSON.parse(data.read("apps/desktop/package.json")).dependencies.synthetic,
      "0.9.0",
    );
    assert.match(
      data.read("apps/desktop/src-tauri/Cargo.toml"),
      /synthetic = "0.9.0"/,
    );
    const lock = data.read("apps/desktop/src-tauri/Cargo.lock");
    assert.equal((lock.match(/version = "0.10.0-rc.1"/g) ?? []).length, 2);
    assert.match(
      lock,
      /name = "synthetic"\nversion = "0.9.0"\nchecksum = "synthetic"/,
    );
    prepareAppVersion(data.root, "0.10.0-rc.1");
    assert.equal(data.read("apps/desktop/src-tauri/Cargo.lock"), lock);
  } finally {
    rmSync(data.root, { recursive: true, force: true });
  }
});

test("a missing helper lock entry fails before any file is changed", () => {
  const data = fixture();
  try {
    writeFileSync(
      join(data.root, "crates/antiburn-remote/Cargo.lock"),
      "version = 4\n",
    );
    assert.throws(
      () => prepareAppVersion(data.root, "0.10.0"),
      /Missing application package/,
    );
    assert.equal(
      data.read("apps/desktop/package.json"),
      data.files["apps/desktop/package.json"],
    );
    assert.equal(
      data.read("apps/desktop/src-tauri/Cargo.toml"),
      data.files["apps/desktop/src-tauri/Cargo.toml"],
    );
  } finally {
    rmSync(data.root, { recursive: true, force: true });
  }
});

test("supports CRLF manifests and lockfiles", () => {
  const data = fixture();
  try {
    for (const [file, content] of Object.entries(data.files)) {
      if (!file.endsWith(".json"))
        writeFileSync(join(data.root, file), content.replaceAll("\n", "\r\n"));
    }
    prepareAppVersion(data.root, "0.10.0");
    assert.match(
      data.read("apps/desktop/src-tauri/Cargo.toml"),
      /version = "0.10.0"\r\n/,
    );
    assert.match(
      data.read("apps/desktop/src-tauri/Cargo.lock"),
      /version = "0.10.0"\r\n/,
    );
  } finally {
    rmSync(data.root, { recursive: true, force: true });
  }
});

test("accepts supported stable and prerelease versions", () => {
  for (const version of [
    "0.9.0",
    "1.2.3",
    "0.10.0-rc.1",
    "1.2.3-alpha-beta.0",
  ]) {
    assert.doesNotThrow(() => validateAppVersion(version));
  }
});

test("rejects invalid versions and command input", () => {
  for (const version of [
    "",
    "v0.9.0",
    "01.2.3",
    "1.2.3-",
    "1.2.3-rc..1",
    "1.2.3-01",
    "1.2.3+meta",
    "1.2.3\n",
    "1.2.3; command",
  ]) {
    assert.throws(() => validateAppVersion(version), /Unsupported/);
  }
});
