import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";
import path from "node:path";
import test from "node:test";

import {
  BUNDLED_ROOT_FILES,
  detectRelease,
  isLegalFile,
  isPureAppReleaseChange,
  isPureEngineReleaseChange,
  listChangedFiles,
  CROSS_WORKSPACE_INPUTS,
  planOutputs,
  resolveDraft,
  resolveStage,
  routeChanges,
  withDependents,
} from "./classify-ci-changes.mjs";
import { WORKSPACES, owningWorkspace } from "./ci-workspaces.mjs";

function json(version, extra = {}) {
  return `${JSON.stringify({ name: "antiburn", version, ...extra }, null, 2)}\n`;
}

function cargoToml(version, dependency = "1") {
  return `[package]\nname = "antiburn"\nversion = "${version}"\n\n[dependencies]\nserde = "${dependency}"\n`;
}

function cargoLock(packageName, version, dependencyVersion = "1.0.0") {
  return `version = 4\n\n[[package]]\nname = "${packageName}"\nversion = "${version}"\ndependencies = [\n "serde",\n]\n\n[[package]]\nname = "serde"\nversion = "${dependencyVersion}"\n`;
}

// The desktop lockfile records the application and the remote helper, and a
// release bumps both. A fixture with one of them cannot show that.
function appCargoLock(appVersion, helperVersion, dependencyVersion = "1.0.0") {
  return `version = 4\n\n[[package]]\nname = "antiburn"\nversion = "${appVersion}"\ndependencies = [\n "antiburn-remote",\n "serde",\n]\n\n[[package]]\nname = "antiburn-remote"\nversion = "${helperVersion}"\ndependencies = [\n "serde",\n]\n\n[[package]]\nname = "serde"\nversion = "${dependencyVersion}"\n`;
}

function remoteCargoToml(version, dependency = "1") {
  return `[workspace]\n\n[package]\nname = "antiburn-remote"\nversion = "${version}"\n\n[dependencies]\nserde = "${dependency}"\n`;
}

const APP_RELEASE_FILES = [
  "CHANGELOG.md",
  "apps/desktop/package.json",
  "apps/desktop/src-tauri/Cargo.lock",
  "apps/desktop/src-tauri/Cargo.toml",
  "apps/desktop/src-tauri/tauri.conf.json",
  "crates/antiburn-remote/Cargo.lock",
  "crates/antiburn-remote/Cargo.toml",
];

/** Every manifest of an app release bumped from `from` to `to`, and nothing else. */
function appReleaseValues(from, to) {
  return {
    "base:apps/desktop/package.json": json(from, { private: true }),
    "head:apps/desktop/package.json": json(to, { private: true }),
    "base:apps/desktop/src-tauri/tauri.conf.json": json(from, {
      productName: "antiburn",
    }),
    "head:apps/desktop/src-tauri/tauri.conf.json": json(to, {
      productName: "antiburn",
    }),
    "base:apps/desktop/src-tauri/Cargo.toml": cargoToml(from),
    "head:apps/desktop/src-tauri/Cargo.toml": cargoToml(to),
    "base:apps/desktop/src-tauri/Cargo.lock": appCargoLock(from, from),
    "head:apps/desktop/src-tauri/Cargo.lock": appCargoLock(to, to),
    "base:crates/antiburn-remote/Cargo.toml": remoteCargoToml(from),
    "head:crates/antiburn-remote/Cargo.toml": remoteCargoToml(to),
    "base:crates/antiburn-remote/Cargo.lock": cargoLock(
      "antiburn-remote",
      from,
    ),
    "head:crates/antiburn-remote/Cargo.lock": cargoLock("antiburn-remote", to),
  };
}

function reader(values) {
  return (ref, file) => values[`${ref}:${file}`];
}

const ALL_WORKSPACES = {
  engine: groupIds("engine"),
  desktop: groupIds("desktop"),
};

function groupIds(group) {
  return WORKSPACES.filter((workspace) => workspace.group === group)
    .map((workspace) => workspace.id)
    .join(",");
}

const OUTPUT_KEYS = [
  "release_app",
  "release_engine",
  "rust_platforms",
  "frontend_platforms",
  "engine_workspaces",
  "desktop_parts",
  "run_engine",
  "run_desktop",
  "run_frontend",
  "run_ci_scripts",
  "run_validator",
  "run_installers",
  "run_licenses",
  "run_advisories",
  "run_remote_build",
  "run_no_analytics_check",
  "run_release_metadata",
  "run_warm_release_cache",
];

function plan(stage, files, release) {
  return planOutputs({ stage, changes: routeChanges(files), release });
}

/** The desktop workspaces of every desktop_parts entry, in matrix order. */
function desktopWorkspaces(outputs) {
  return JSON.parse(outputs.desktop_parts)
    .map((part) => part.workspaces)
    .join(",");
}

/** Every run_* output that the plan selects. */
function selectedJobs(outputs) {
  return Object.entries(outputs)
    .filter(([key, value]) => key.startsWith("run_") && value === "true")
    .map(([key]) => key);
}

function platformNames(value) {
  return JSON.parse(value).map((platform) => platform.name);
}

test("routes documentation without compiling application code", () => {
  const outputs = plan("ready", ["README.md", "docs/runbooks/release.md"]);
  assert.deepEqual(selectedJobs(outputs), []);
  assert.equal(outputs.engine_workspaces, "");
  assert.equal(outputs.desktop_parts, "[]");
  assert.equal(outputs.release_app, "false");
  assert.equal(outputs.release_engine, "false");
});

test("routes engine changes through both Rust matrices", () => {
  const outputs = plan("ready", ["crates/antiburn-local/src/lib.rs"]);
  assert.equal(outputs.run_engine, "true");
  assert.equal(outputs.run_desktop, "true");
  assert.equal(outputs.run_frontend, "false");
});

test("routes frontend changes without Rust matrices", () => {
  const outputs = plan("ready", ["apps/desktop/src/App.tsx"]);
  assert.equal(outputs.run_frontend, "true");
  assert.equal(outputs.run_engine, "false");
  assert.equal(outputs.run_desktop, "false");
});

test("fails closed for workflow and unknown paths", () => {
  for (const file of [
    ".github/workflows/ci.yml",
    "unclassified.config",
    ".antiburn/checks/new-check.yml",
    "crates/unknown-crate/src/lib.rs",
    "apps/desktop/src-tauri/crates/new-crate/src/lib.rs",
    "apps/desktop/src-tauri/vendor/foo/src/lib.rs",
    "apps/desktop/src-tauri/vendor/foo/Cargo.toml",
    "scripts/ci-rust.mjs",
    "scripts/classify-ci-changes.mjs",
    "scripts/package-remote-helper.sh",
  ]) {
    const outputs = plan("ready", [file]);
    assert.equal(outputs.engine_workspaces, ALL_WORKSPACES.engine, file);
    assert.equal(desktopWorkspaces(outputs), ALL_WORKSPACES.desktop, file);
    for (const key of [
      "run_engine",
      "run_desktop",
      "run_frontend",
      "run_ci_scripts",
      "run_installers",
      "run_licenses",
      "run_remote_build",
    ]) {
      assert.equal(outputs[key], "true", `${file} ${key}`);
    }
  }
});

test("routes dependency manifests to their workspace instead of a full run", () => {
  const lock = plan("ready", ["pnpm-lock.yaml"]);
  assert.equal(lock.run_frontend, "true");
  assert.equal(lock.run_engine, "false");
  assert.equal(lock.run_desktop, "false");
  assert.equal(lock.run_installers, "false");

  const manifest = plan("ready", ["apps/desktop/src-tauri/Cargo.toml"]);
  assert.equal(manifest.run_engine, "false");
  assert.equal(manifest.run_desktop, "true");
  assert.equal(desktopWorkspaces(manifest), "shell");
  assert.equal(manifest.run_installers, "false");
});

test("includes deleted paths in routing instead of silently skipping them", () => {
  let receivedArguments;
  const files = listChangedFiles("base", "head", (...arguments_) => {
    receivedArguments = arguments_;
    return "apps/desktop/src/removed.tsx\n";
  });

  assert.deepEqual(receivedArguments, ["diff", "--name-only", "base", "head"]);
  assert.equal(plan("ready", files).run_frontend, "true");
});

/** The ready-stage outputs for `files`, with release detection. */
function planWithRelease(files, readAtRef) {
  return plan("ready", files, detectRelease(files, readAtRef));
}

test("recognizes an app release when every executable manifest changes only version", () => {
  const values = appReleaseValues("1.0.0", "1.0.1");
  assert.equal(isPureAppReleaseChange(APP_RELEASE_FILES, reader(values)), true);
  assert.equal(
    planWithRelease(APP_RELEASE_FILES, reader(values)).release_app,
    "true",
  );
});

test("rejects an app release that leaves the remote helper behind", () => {
  const values = {
    ...appReleaseValues("1.0.0", "1.0.1"),
    "head:crates/antiburn-remote/Cargo.toml": remoteCargoToml("1.0.0"),
    "head:crates/antiburn-remote/Cargo.lock": cargoLock(
      "antiburn-remote",
      "1.0.0",
    ),
    "head:apps/desktop/src-tauri/Cargo.lock": appCargoLock("1.0.1", "1.0.0"),
  };
  assert.equal(
    isPureAppReleaseChange(APP_RELEASE_FILES, reader(values)),
    false,
  );
});

test("rejects an app release that smuggles a dependency change into Cargo files", () => {
  const values = {
    ...appReleaseValues("1.0.0", "1.0.1"),
    "head:apps/desktop/src-tauri/Cargo.toml": cargoToml("1.0.1", "2"),
    "head:apps/desktop/src-tauri/Cargo.lock": appCargoLock(
      "1.0.1",
      "1.0.1",
      "2.0.0",
    ),
  };
  assert.equal(
    isPureAppReleaseChange(APP_RELEASE_FILES, reader(values)),
    false,
  );
  const outputs = planWithRelease(APP_RELEASE_FILES, reader(values));
  assert.equal(outputs.release_app, "false");
  assert.equal(outputs.run_desktop, "true");
});

test("recognizes an engine release across both lockfiles", () => {
  const files = [
    "apps/desktop/src-tauri/Cargo.lock",
    "crates/antiburn-local/CHANGELOG.md",
    "crates/antiburn-local/Cargo.lock",
    "crates/antiburn-local/Cargo.toml",
  ];
  const values = {
    "base:crates/antiburn-local/Cargo.toml": cargoToml("1.0.0"),
    "head:crates/antiburn-local/Cargo.toml": cargoToml("1.0.1"),
    "base:crates/antiburn-local/Cargo.lock": cargoLock(
      "antiburn-local",
      "1.0.0",
    ),
    "head:crates/antiburn-local/Cargo.lock": cargoLock(
      "antiburn-local",
      "1.0.1",
    ),
    "base:apps/desktop/src-tauri/Cargo.lock": cargoLock(
      "antiburn-local",
      "1.0.0",
    ),
    "head:apps/desktop/src-tauri/Cargo.lock": cargoLock(
      "antiburn-local",
      "1.0.1",
    ),
  };
  assert.equal(isPureEngineReleaseChange(files, reader(values)), true);
  assert.equal(planWithRelease(files, reader(values)).release_engine, "true");
});

test("resolves the stage from the event, the draft flag, and live labels", () => {
  const cases = [
    [{ eventName: "pull_request", draft: true, labels: [] }, "draft"],
    [{ eventName: "pull_request", draft: true, labels: ["ci:full"] }, "ready"],
    [
      { eventName: "pull_request", draft: true, labels: ["bug", "ci:full"] },
      "ready",
    ],
    [{ eventName: "pull_request", draft: true, labels: ["bug"] }, "draft"],
    [{ eventName: "pull_request", draft: false, labels: [] }, "ready"],
    [{ eventName: "push", ref: "refs/heads/main" }, "main"],
    [{ eventName: "schedule", ref: "refs/heads/main" }, "nightly"],
    [{ eventName: "workflow_dispatch", ref: "refs/heads/main" }, "nightly"],
  ];
  for (const [input, expected] of cases) {
    assert.equal(resolveStage(input), expected, JSON.stringify(input));
  }
});

test("writes exactly the documented output keys as strings", () => {
  for (const stage of ["draft", "ready", "main", "nightly"]) {
    const outputs = plan(stage, ["apps/desktop/src/App.tsx"]);
    assert.deepEqual(Object.keys(outputs), OUTPUT_KEYS);
    for (const [key, value] of Object.entries(outputs)) {
      assert.equal(typeof value, "string", key);
      assert.equal(value.includes("\n"), false, key);
    }
  }
});

test("chooses platforms per stage", () => {
  const expected = {
    draft: { rust: ["linux"], frontend: ["linux"] },
    ready: {
      rust: ["linux", "windows", "macos"],
      frontend: ["linux", "windows"],
    },
    main: {
      rust: ["linux", "linux-arm64", "windows", "windows-arm64", "macos"],
      frontend: ["linux", "windows"],
    },
    nightly: {
      rust: ["linux", "linux-arm64", "windows", "windows-arm64", "macos"],
      frontend: ["linux", "windows"],
    },
  };
  for (const [stage, platforms] of Object.entries(expected)) {
    const outputs = plan(stage, ["crates/antiburn-local/src/lib.rs"]);
    assert.deepEqual(platformNames(outputs.rust_platforms), platforms.rust);
    assert.deepEqual(
      platformNames(outputs.frontend_platforms),
      platforms.frontend,
    );
  }
  assert.deepEqual(JSON.parse(plan("main", []).rust_platforms), [
    { name: "linux", os: "ubuntu-24.04" },
    { name: "linux-arm64", os: "ubuntu-24.04-arm" },
    { name: "windows", os: "windows-latest" },
    { name: "windows-arm64", os: "windows-11-arm" },
    { name: "macos", os: "macos-latest" },
  ]);
});

test("selects transitive dependents of a changed workspace", () => {
  assert.deepEqual(withDependents(["engine"]), [
    "engine",
    "remote",
    "shell",
    "checks-store",
    "cli",
  ]);
  assert.deepEqual(withDependents(["remote"]), ["remote", "shell"]);
  assert.deepEqual(withDependents(["hud"]), ["shell", "hud"]);
  assert.deepEqual(withDependents(["tray-icon"]), ["shell", "tray-icon"]);
  assert.deepEqual(withDependents(["shell"]), ["shell"]);
});

test("routes Rust paths to their workspaces", () => {
  const cases = [
    [
      "crates/antiburn-local/src/lib.rs",
      "engine,remote,checks-store,cli",
      "shell",
    ],
    ["crates/antiburn-remote/src/main.rs", "remote", "shell"],
    ["apps/desktop/src-tauri/src/lib.rs", "", "shell"],
    ["apps/desktop/src-tauri/tauri.conf.json", "", "shell"],
    ["apps/desktop/src-tauri/crates/hud/src/lib.rs", "", "shell,hud"],
    ["apps/desktop/src-tauri/crates/trace/src/lib.rs", "", "shell,trace"],
    [
      "apps/desktop/src-tauri/crates/main-window/src/lib.rs",
      "",
      "shell,main-window",
    ],
    [
      "apps/desktop/src-tauri/crates/anchored-window/src/lib.rs",
      "",
      "shell,anchored-window",
    ],
    ["apps/desktop/src-tauri/crates/nudge/src/lib.rs", "", "shell,nudge"],
    ["apps/desktop/src-tauri/crates/sound/src/lib.rs", "", "shell,sound"],
    [
      "apps/desktop/src-tauri/vendor/tray-icon/src/lib.rs",
      "",
      "shell,tray-icon",
    ],
  ];
  for (const [file, engine, desktop] of cases) {
    const outputs = plan("ready", [file]);
    assert.equal(outputs.engine_workspaces, engine, file);
    assert.equal(desktopWorkspaces(outputs), desktop, file);
    assert.equal(outputs.run_engine, String(engine !== ""), file);
    assert.equal(outputs.run_desktop, "true", file);
    assert.equal(outputs.run_frontend, "false", file);
    assert.equal(outputs.run_licenses, "false", file);
    assert.equal(outputs.run_installers, "false", file);
  }
});

test("routes a lockfile change to its workspace and the license checks", () => {
  for (const [file, engine, desktop] of [
    [
      "crates/antiburn-local/Cargo.lock",
      "engine,remote,checks-store,cli",
      "shell",
    ],
    [
      "crates/antiburn-local/deny.toml",
      "engine,remote,checks-store,cli",
      "shell",
    ],
    ["crates/antiburn-remote/Cargo.toml", "remote", "shell"],
    ["apps/desktop/src-tauri/Cargo.lock", "", "shell"],
    ["apps/desktop/src-tauri/about.toml", "", "shell"],
    ["apps/desktop/src-tauri/crates/hud/Cargo.lock", "", "shell,hud"],
    [
      "apps/desktop/src-tauri/vendor/tray-icon/Cargo.toml",
      "",
      "shell,tray-icon",
    ],
  ]) {
    const outputs = plan("ready", [file]);
    assert.equal(outputs.run_installers, "false", file);
    assert.equal(outputs.engine_workspaces, engine, file);
    assert.equal(desktopWorkspaces(outputs), desktop, file);
    assert.equal(outputs.run_licenses, "true", file);
    assert.equal(outputs.run_frontend, "false", file);
  }
});

test("routes frontend, Node manifest, and license inputs", () => {
  const frontend = plan("ready", ["apps/desktop/src/App.tsx"]);
  assert.equal(frontend.run_frontend, "true");
  assert.equal(frontend.run_licenses, "false");
  assert.equal(frontend.run_ci_scripts, "false");
  assert.equal(frontend.run_engine, "false");
  assert.equal(frontend.run_desktop, "false");

  const appManifest = plan("ready", ["apps/desktop/package.json"]);
  assert.equal(appManifest.run_frontend, "true");
  assert.equal(appManifest.run_licenses, "true");
  assert.equal(appManifest.run_ci_scripts, "false");

  for (const file of [
    "package.json",
    "pnpm-lock.yaml",
    "pnpm-workspace.yaml",
  ]) {
    const outputs = plan("ready", [file]);
    assert.equal(outputs.run_frontend, "true", file);
    assert.equal(outputs.run_ci_scripts, "true", file);
    assert.equal(outputs.run_licenses, "true", file);
    assert.equal(outputs.run_installers, "false", file);
    assert.equal(outputs.run_desktop, "false", file);
  }

  for (const file of [
    "scripts/third-party-notices.mjs",
    "scripts/third-party-license-overrides/react-remove-scroll-bar-2.3.8.txt",
  ]) {
    const outputs = plan("ready", [file]);
    assert.equal(outputs.run_licenses, "true", file);
    assert.equal(outputs.run_frontend, "false", file);
    assert.equal(outputs.run_desktop, "false", file);
  }
});

test("routes the root legal files to the frontend and the shell", () => {
  for (const [file, licenses] of [
    ["LICENSE", "false"],
    ["NOTICE", "false"],
    ["THIRD_PARTY_NOTICES", "true"],
  ]) {
    const outputs = plan("ready", [file]);
    assert.equal(outputs.run_frontend, "true", file);
    assert.equal(outputs.run_desktop, "true", file);
    assert.equal(desktopWorkspaces(outputs), "shell", file);
    assert.equal(outputs.run_engine, "false", file);
    assert.equal(outputs.run_licenses, licenses, file);
    assert.equal(outputs.run_ci_scripts, "false", file);
    assert.equal(outputs.run_installers, "false", file);
  }
  assert.deepEqual(Object.keys(BUNDLED_ROOT_FILES).sort(), [
    "LICENSE",
    "NOTICE",
    "THIRD_PARTY_NOTICES",
  ]);
  // A diff with only a root legal file is not documentation-only.
  assert.deepEqual(selectedJobs(plan("ready", ["LICENSE"])), [
    "run_desktop",
    "run_frontend",
  ]);
  const mixed = plan("ready", ["README.md", "NOTICE"]);
  assert.equal(mixed.run_frontend, "true");
  assert.equal(desktopWorkspaces(mixed), "shell");
});

test("routes installer files to the installer tests only", () => {
  for (const file of [
    "install.sh",
    "install.ps1",
    "scripts/install-sh.test.mjs",
    "scripts/install-ps1.test.ps1",
    "scripts/windows-signing.ps1",
    "scripts/windows-signing.test.ps1",
    "scripts/setup-windows-signing.ps1",
  ]) {
    const outputs = plan("ready", [file]);
    assert.equal(outputs.run_installers, "true", file);
    assert.equal(outputs.run_ci_scripts, "false", file);
    assert.equal(outputs.run_licenses, "false", file);
    assert.equal(outputs.run_frontend, "false", file);
    assert.equal(outputs.run_engine, "false", file);
    assert.equal(outputs.run_desktop, "false", file);
  }
});

test("routes other scripts to the CI script tests only", () => {
  const outputs = plan("ready", ["scripts/wait-for-main-ci.mjs"]);
  assert.equal(outputs.run_ci_scripts, "true");
  assert.equal(outputs.run_frontend, "false");
  assert.equal(outputs.run_engine, "false");
  assert.equal(outputs.run_desktop, "false");
  assert.equal(outputs.run_installers, "false");
  assert.equal(outputs.run_licenses, "false");
});

test("routes Burn Check documentation changes to the validator", () => {
  for (const file of [
    "docs/burn-check/README.md",
    "docs/burn-check/tests/new-case.yml",
    "docs/burn-check/scripts/validate-checks.mjs",
    "docs/burn-check/package-lock.json",
  ]) {
    const outputs = plan("ready", [file]);
    assert.equal(outputs.run_validator, "true", file);
    assert.deepEqual(selectedJobs(outputs), ["run_validator"], file);
  }

  const unrelatedDocs = plan("ready", ["docs/session-coverage.md"]);
  assert.equal(unrelatedDocs.run_validator, "false");
});

test("validates canonical repository checks as well as their Rust readers", () => {
  const outputs = plan("ready", [".antiburn/checks/fast-mode-value.yml"]);
  assert.equal(outputs.engine_workspaces, "checks,checks-store,cli");
  assert.equal(desktopWorkspaces(outputs), "shell");
  assert.equal(outputs.run_validator, "true");
  assert.equal(outputs.run_frontend, "false");
});

test("selects every job for workflow changes", () => {
  const outputs = plan("ready", [".github/workflows/ci.yml"]);
  assert.equal(outputs.engine_workspaces, ALL_WORKSPACES.engine);
  assert.equal(desktopWorkspaces(outputs), ALL_WORKSPACES.desktop);
  for (const key of [
    "run_frontend",
    "run_ci_scripts",
    "run_validator",
    "run_installers",
    "run_licenses",
    "run_remote_build",
  ]) {
    assert.equal(outputs[key], "true", key);
  }
  assert.equal(outputs.run_advisories, "false");
  assert.equal(outputs.run_no_analytics_check, "false");
});

test("selects nothing for documentation", () => {
  const outputs = plan("ready", ["README.md", "docs/runbooks/release.md"]);
  for (const [key, value] of Object.entries(outputs)) {
    if (key.startsWith("run_")) {
      assert.equal(value, "false", key);
    }
  }
  assert.equal(outputs.engine_workspaces, "");
  assert.equal(desktopWorkspaces(outputs), "");
});

test("a draft keeps the selected workspaces but skips installers and licenses", () => {
  const outputs = plan("draft", [".github/workflows/ci.yml"]);
  assert.equal(outputs.engine_workspaces, ALL_WORKSPACES.engine);
  assert.equal(desktopWorkspaces(outputs), ALL_WORKSPACES.desktop);
  assert.equal(outputs.run_frontend, "true");
  assert.equal(outputs.run_ci_scripts, "true");
  assert.equal(outputs.run_installers, "false");
  assert.equal(outputs.run_licenses, "false");
  assert.equal(outputs.run_remote_build, "false");
  assert.equal(outputs.run_no_analytics_check, "false");
  assert.equal(outputs.run_advisories, "false");
});

test("builds the remote helper after draft when the engine or the helper changes", () => {
  assert.equal(
    plan("ready", ["crates/antiburn-local/src/lib.rs"]).run_remote_build,
    "true",
  );
  assert.equal(
    plan("ready", ["apps/desktop/src-tauri/src/lib.rs"]).run_remote_build,
    "false",
  );
  assert.equal(
    plan("draft", ["crates/antiburn-local/src/lib.rs"]).run_remote_build,
    "false",
  );
  assert.equal(
    plan("ready", ["crates/antiburn-remote/src/main.rs"]).run_remote_build,
    "true",
  );
  assert.equal(
    plan("draft", ["crates/antiburn-remote/src/main.rs"]).run_remote_build,
    "false",
  );
  assert.equal(
    plan("main", ["crates/antiburn-local/src/lib.rs"]).run_remote_build,
    "true",
  );
  assert.equal(
    plan("main", ["apps/desktop/src-tauri/src/lib.rs"]).run_remote_build,
    "false",
  );
});

test("runs the shell Clippy pass without analytics only on main and nightly", () => {
  const shell = ["apps/desktop/src-tauri/src/lib.rs"];
  assert.equal(plan("draft", shell).run_no_analytics_check, "false");
  assert.equal(plan("ready", shell).run_no_analytics_check, "false");
  assert.equal(plan("main", shell).run_no_analytics_check, "true");
  assert.equal(plan("nightly", []).run_no_analytics_check, "true");
  assert.equal(
    plan("main", ["apps/desktop/src/App.tsx"]).run_no_analytics_check,
    "false",
  );
});

test("nightly selects everything regardless of the diff", () => {
  for (const files of [[], ["README.md"], ["apps/desktop/src/App.tsx"]]) {
    const outputs = plan("nightly", files, { app: true });
    assert.equal(JSON.parse(outputs.rust_platforms).length, 5);
    assert.equal(outputs.engine_workspaces, ALL_WORKSPACES.engine);
    assert.equal(desktopWorkspaces(outputs), ALL_WORKSPACES.desktop);
    for (const [key, value] of Object.entries(outputs)) {
      if (!key.startsWith("run_")) continue;
      const expected = [
        "run_release_metadata",
        "run_warm_release_cache",
      ].includes(key)
        ? "false"
        : "true";
      assert.equal(value, expected, key);
    }
    assert.equal(outputs.release_app, "false");
  }
});

test("a release bump selects only release metadata", () => {
  const app = plan("main", APP_RELEASE_FILES, { app: true, engine: false });
  assert.equal(app.release_app, "true");
  assert.equal(app.run_release_metadata, "true");
  assert.equal(app.run_warm_release_cache, "true");
  assert.equal(app.engine_workspaces, "");
  assert.equal(desktopWorkspaces(app), "");
  for (const key of [
    "run_engine",
    "run_desktop",
    "run_frontend",
    "run_ci_scripts",
    "run_installers",
    "run_licenses",
    "run_remote_build",
    "run_no_analytics_check",
  ]) {
    assert.equal(app[key], "false", key);
  }

  const readyApp = plan("ready", APP_RELEASE_FILES, { app: true });
  assert.equal(readyApp.run_release_metadata, "true");
  assert.equal(readyApp.run_warm_release_cache, "false");

  const engine = plan("main", ["crates/antiburn-local/Cargo.toml"], {
    app: false,
    engine: true,
  });
  assert.equal(engine.release_engine, "true");
  assert.equal(engine.run_release_metadata, "true");
  assert.equal(engine.run_warm_release_cache, "false");
  assert.equal(engine.run_engine, "false");
});

function routeWithWorkspaces(workspaces) {
  return { ...routeChanges([]), workspaces };
}

test("splits the desktop workspaces into a shell part and a crates part", () => {
  const cases = [
    [["shell"], [{ name: "shell", workspaces: "shell", linux_deps: true }]],
    [
      ["hud", "sound"],
      [{ name: "crates", workspaces: "hud,sound", linux_deps: true }],
    ],
    [
      ["engine", "shell", "trace", "tray-icon"],
      [
        { name: "shell", workspaces: "shell", linux_deps: true },
        { name: "crates", workspaces: "trace,tray-icon", linux_deps: true },
      ],
    ],
    [["engine", "remote"], []],
    [[], []],
  ];
  for (const [workspaces, expected] of cases) {
    const outputs = planOutputs({
      stage: "ready",
      changes: routeWithWorkspaces(workspaces),
    });
    assert.ok(!outputs.desktop_parts.includes("\n"));
    assert.deepEqual(JSON.parse(outputs.desktop_parts), expected, workspaces);
  }
});

test("desktop_parts asks for the Linux system packages only when a part needs them", () => {
  // The trace crate has no system dependencies.
  assert.deepEqual(
    JSON.parse(
      plan("ready", ["apps/desktop/src-tauri/crates/trace/src/lib.rs"])
        .desktop_parts,
    ),
    [
      { name: "shell", workspaces: "shell", linux_deps: true },
      { name: "crates", workspaces: "trace", linux_deps: false },
    ],
  );
  assert.deepEqual(
    JSON.parse(
      planOutputs({
        stage: "ready",
        changes: routeWithWorkspaces(["trace"]),
      }).desktop_parts,
    ),
    [{ name: "crates", workspaces: "trace", linux_deps: false }],
  );
  // A part needs the packages when one of its workspaces needs them.
  for (const part of JSON.parse(plan("nightly", []).desktop_parts)) {
    const expected = WORKSPACES.some(
      (workspace) =>
        part.workspaces.split(",").includes(workspace.id) &&
        workspace.linuxSystemDeps === true,
    );
    assert.equal(part.linux_deps, expected, part.name);
    assert.equal(typeof part.linux_deps, "boolean", part.name);
  }
});

test("desktop_parts follows the routed workspaces", () => {
  assert.deepEqual(
    JSON.parse(
      plan("ready", ["apps/desktop/src-tauri/src/lib.rs"]).desktop_parts,
    ),
    [{ name: "shell", workspaces: "shell", linux_deps: true }],
  );
  assert.deepEqual(
    JSON.parse(
      plan("ready", ["apps/desktop/src-tauri/crates/hud/src/lib.rs"])
        .desktop_parts,
    ),
    [
      { name: "shell", workspaces: "shell", linux_deps: true },
      { name: "crates", workspaces: "hud", linux_deps: true },
    ],
  );
  assert.equal(plan("ready", ["README.md"]).desktop_parts, "[]");
  assert.equal(plan("ready", ["apps/desktop/src/App.tsx"]).desktop_parts, "[]");
  assert.deepEqual(JSON.parse(plan("nightly", []).desktop_parts), [
    { name: "shell", workspaces: "shell", linux_deps: true },
    {
      name: "crates",
      workspaces: ALL_WORKSPACES.desktop.replace(/^shell,/, ""),
      linux_deps: true,
    },
  ]);
});

test("routes files that only the shell reads from outside its workspace to the shell", () => {
  for (const [file, frontend] of [
    ["docs/analytics.md", false],
    ["apps/desktop/interface-scale.json", true],
    ["apps/desktop/src/views/SettingsView.tsx", true],
  ]) {
    const outputs = plan("ready", [file]);
    assert.equal(outputs.run_desktop, "true", file);
    assert.equal(outputs.run_installers, "false", file);
    assert.equal(desktopWorkspaces(outputs), "shell", file);
    assert.equal(outputs.engine_workspaces, "", file);
    assert.equal(outputs.run_frontend, String(frontend), file);
  }
  const mixed = plan("ready", ["README.md", "docs/analytics.md"]);
  assert.equal(mixed.run_desktop, "true");
  assert.equal(desktopWorkspaces(mixed), "shell");
});

test("routes engine fixtures that other workspaces embed to those workspaces", () => {
  const outputs = plan("ready", [
    "crates/antiburn-local/tests/fixtures/codex_characterization/task_complete_errors.jsonl",
  ]);
  assert.equal(outputs.engine_workspaces, "engine,remote,checks-store,cli");
  assert.equal(desktopWorkspaces(outputs), "shell");
});

test("routes legal files inside a workspace to the license checks", () => {
  for (const [file, desktop] of [
    ["apps/desktop/src-tauri/vendor/tray-icon/LICENSE-MIT", "shell,tray-icon"],
    ["apps/desktop/src-tauri/vendor/tray-icon/LICENSE.spdx", "shell,tray-icon"],
    ["apps/desktop/src-tauri/crates/sound/COPYING", "shell,sound"],
    ["crates/antiburn-remote/NOTICE", "shell"],
  ]) {
    const outputs = plan("ready", [file]);
    assert.equal(outputs.run_installers, "false", file);
    assert.equal(desktopWorkspaces(outputs), desktop, file);
    assert.equal(outputs.run_licenses, "true", file);
  }
  assert.equal(
    plan("draft", ["apps/desktop/src-tauri/vendor/tray-icon/LICENSE-MIT"])
      .run_licenses,
    "false",
  );
});

// Engine test helpers that read a file from the repository docs/ directory.
// The scan checks that each helper still resolves docs/ the same way.
const DOC_READERS = [
  {
    source: "crates/antiburn-local/tests/check_coverage_contract.rs",
    call: /\bcoverage_document\(\s*"([^"]+)"\s*\)/g,
    resolves: 'manifest_dir.join("../../docs")',
  },
];

// Each workspace reads files with include_str! and include_bytes!, and its
// tests read files from paths relative to CARGO_MANIFEST_DIR. A file outside
// every workspace must be in CROSS_WORKSPACE_INPUTS with each reader, or a
// change to it skips the tests that read it.
test("lists every file that a workspace reads outside the Cargo workspaces", () => {
  const root = execFileSync("git", ["rev-parse", "--show-toplevel"], {
    encoding: "utf8",
  }).trim();
  const sources = execFileSync(
    "git",
    ["ls-files", "--", "crates/**/*.rs", "apps/desktop/src-tauri/**/*.rs"],
    { cwd: root, encoding: "utf8" },
  )
    .split("\n")
    .filter(Boolean);
  const embed =
    /include_(?:str|bytes)!\s*\(\s*(?:concat!\s*\(\s*env!\s*\(\s*"([A-Z_]+)"\s*\)\s*,\s*"([^"]+)"|"([^"]+)")/g;
  // A string such as "/../../../docs/support.md" that the code appends to
  // CARGO_MANIFEST_DIR.
  const manifestRelative = /"(\/\.\.\/[^"]+)"/g;
  const readers = new Map();
  const record = (target, workspace) => {
    if (owningWorkspace(target) !== null) return;
    if (!readers.has(target)) readers.set(target, new Set());
    readers.get(target).add(workspace.id);
  };
  let embeds = 0;
  let runtimeReads = 0;
  for (const source of sources) {
    const workspace = owningWorkspace(source);
    const dir = workspace.directory;
    const text = readFileSync(path.join(root, source), "utf8");
    for (const match of text.matchAll(embed)) {
      const [, variable, joined, relative] = match;
      // OUT_DIR holds build output, not a repository file.
      if (variable === "OUT_DIR") continue;
      embeds += 1;
      record(
        path.posix.normalize(
          variable === undefined
            ? path.posix.join(path.posix.dirname(source), relative)
            : path.posix.join(dir, joined),
        ),
        workspace,
      );
    }
    if (text.includes("CARGO_MANIFEST_DIR")) {
      for (const [, relative] of text.matchAll(manifestRelative)) {
        runtimeReads += 1;
        record(path.posix.normalize(path.posix.join(dir, relative)), workspace);
      }
    }
  }
  for (const reader of DOC_READERS) {
    const text = readFileSync(path.join(root, reader.source), "utf8");
    assert.ok(
      text.includes(reader.resolves),
      `${reader.source}: the docs directory changed; update DOC_READERS`,
    );
    const workspace = owningWorkspace(reader.source);
    for (const [, name] of text.matchAll(reader.call)) {
      runtimeReads += 1;
      record(`docs/${name}`, workspace);
    }
  }
  assert.ok(embeds > 100, `found only ${embeds} embeds`);
  assert.ok(runtimeReads > 10, `found only ${runtimeReads} runtime reads`);
  const found = Object.fromEntries(
    [...readers]
      .sort(([a], [b]) => a.localeCompare(b))
      .map(([target, ids]) => [target, [...ids].sort()]),
  );
  const listed = Object.fromEntries(
    Object.entries(CROSS_WORKSPACE_INPUTS)
      .sort(([a], [b]) => a.localeCompare(b))
      .map(([target, ids]) => [target, [...ids].sort()]),
  );
  assert.deepEqual(listed, found);
});

test("runs the readers of files read outside the workspaces", () => {
  for (const [file, engine, desktop] of [
    ["docs/support.md", "engine,remote,checks-store,cli", "shell"],
    ["docs/session-coverage.md", "engine,remote,checks-store,cli", "shell"],
    ["docs/check-coverage.md", "engine,remote,checks-store,cli", "shell"],
    ["docs/privacy-policy.md", "", "shell"],
    [
      "plugins/antiburn-live/hooks/catalog.ts",
      "engine,remote,checks-store,cli",
      "shell",
    ],
    ["apps/desktop/src/views/settings/PrivacyPane.tsx", "", "shell"],
  ]) {
    const outputs = plan("ready", [file]);
    assert.equal(outputs.engine_workspaces, engine, file);
    assert.equal(desktopWorkspaces(outputs), desktop, file);
    assert.equal(outputs.run_validator, "false", file);
  }
});

test("runs desktop readers and the validator for canonical V2 definitions", () => {
  for (const file of [
    ".antiburn/checks/fast-mode-value.yml",
    ".antiburn/checks/over-exploring.yml",
    ".antiburn/checks/scope-creep.yml",
  ]) {
    const outputs = plan("ready", [file]);
    assert.equal(outputs.engine_workspaces, "checks,checks-store,cli", file);
    assert.equal(desktopWorkspaces(outputs), "shell", file);
    assert.equal(outputs.run_validator, "true", file);
  }
});

test("runs new inventory definition readers and schema validation", () => {
  for (const file of [
    ".antiburn/checks/ignored-instructions.yml",
    ".antiburn/checks/skill-opportunities.yml",
  ]) {
    const outputs = plan("ready", [file]);
    assert.equal(outputs.engine_workspaces, "checks,checks-store,cli", file);
    assert.equal(desktopWorkspaces(outputs), "shell", file);
    assert.equal(outputs.run_validator, "true", file);
  }
});

test("prefers the live draft state over the event payload", () => {
  const eventSaysDraft = () => true;
  const eventSaysReady = () => false;
  assert.equal(resolveDraft("false", eventSaysDraft), false);
  assert.equal(resolveDraft("true", eventSaysReady), true);
  assert.equal(resolveDraft("", eventSaysDraft), true);
  assert.equal(resolveDraft(undefined, eventSaysReady), false);
});

test("a change to the shared workspace registry runs every job", () => {
  const outputs = plan("ready", ["scripts/ci-workspaces.mjs"]);
  assert.equal(outputs.engine_workspaces, ALL_WORKSPACES.engine);
  assert.equal(desktopWorkspaces(outputs), ALL_WORKSPACES.desktop);
  assert.equal(outputs.run_ci_scripts, "true");
});

test("main and nightly use the same platform lists", () => {
  const main = plan("main", ["crates/antiburn-local/src/lib.rs"]);
  const nightly = plan("nightly", ["crates/antiburn-local/src/lib.rs"]);
  assert.equal(main.rust_platforms, nightly.rust_platforms);
  assert.equal(main.frontend_platforms, nightly.frontend_platforms);
  assert.equal(JSON.parse(main.rust_platforms).length, 5);
});

test("uses one rule for legal files, in any case", () => {
  for (const name of Object.keys(BUNDLED_ROOT_FILES)) {
    assert.ok(isLegalFile(name), name);
  }
  for (const name of [
    "LICENSE-APACHE",
    "License.md",
    "licence",
    "COPYRIGHT",
    "copying",
    "NOTICE.txt",
    "notice",
    "THIRD_PARTY_NOTICES",
    "x.SPDX",
  ]) {
    assert.ok(isLegalFile(name), name);
  }
  for (const name of ["Cargo.toml", "README.md", "licenses.rs"]) {
    assert.equal(isLegalFile(name), false, name);
  }
  const outputs = plan("ready", [
    "apps/desktop/src-tauri/crates/sound/Copyright",
  ]);
  assert.equal(outputs.run_licenses, "true");
});

test("routes desktop files by workspace before the frontend rule", () => {
  const shell = plan("ready", ["apps/desktop/src-tauri/src/lib.rs"]);
  assert.equal(desktopWorkspaces(shell), "shell");
  assert.equal(shell.run_frontend, "false");
  const sound = plan("ready", [
    "apps/desktop/src-tauri/crates/sound/src/lib.rs",
  ]);
  assert.equal(desktopWorkspaces(sound), "shell,sound");
  const frontend = plan("ready", ["apps/desktop/src/App.tsx"]);
  assert.equal(desktopWorkspaces(frontend), "");
  assert.equal(frontend.run_frontend, "true");
  for (const file of [
    "apps/desktop/src-tauri/crates/new-crate/src/lib.rs",
    "apps/desktop/src-tauri/vendor/new-crate/Cargo.toml",
  ]) {
    const outputs = plan("ready", [file]);
    assert.equal(desktopWorkspaces(outputs), ALL_WORKSPACES.desktop, file);
  }
});

test("checks advisories after draft when a pull request changes a Cargo.lock", () => {
  for (const file of [
    "crates/antiburn-local/Cargo.lock",
    "crates/antiburn-remote/Cargo.lock",
    "apps/desktop/src-tauri/Cargo.lock",
    "apps/desktop/src-tauri/vendor/tray-icon/Cargo.lock",
  ]) {
    for (const stage of ["ready", "main"]) {
      const outputs = plan(stage, [file]);
      assert.equal(outputs.run_advisories, "true", `${stage} ${file}`);
      // The advisories run in the licenses job.
      assert.equal(outputs.run_licenses, "true", `${stage} ${file}`);
    }
    assert.equal(plan("draft", [file]).run_advisories, "false", file);
  }
  for (const file of [
    "crates/antiburn-local/Cargo.toml",
    "apps/desktop/src-tauri/src/lib.rs",
    ".github/workflows/ci.yml",
    "pnpm-lock.yaml",
  ]) {
    assert.equal(plan("ready", [file]).run_advisories, "false", file);
  }
});

test("routes the session check reader and CLI to their dependent workspaces", () => {
  assert.deepEqual(withDependents(["checks-store"]), [
    "shell",
    "checks-store",
    "cli",
  ]);
  const reader = plan("ready", ["crates/antiburn-checks-store/src/lib.rs"]);
  assert.equal(reader.engine_workspaces, "checks-store,cli");
  assert.equal(desktopWorkspaces(reader), "shell");
  const cli = plan("ready", ["apps/cli/src/delivery.rs"]);
  assert.equal(cli.engine_workspaces, "cli");
  assert.equal(desktopWorkspaces(cli), "");
});

test("runtime policy changes check the library and desktop", () => {
  const runtime = plan("ready", [
    "crates/antiburn-cli-runtime/src/lifecycle.rs",
  ]);
  assert.equal(runtime.engine_workspaces, "cli-runtime");
  assert.equal(desktopWorkspaces(runtime), "shell");
});
