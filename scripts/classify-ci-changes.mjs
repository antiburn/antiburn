#!/usr/bin/env node
import { appendFileSync, existsSync, readFileSync } from "node:fs";
import { execFileSync } from "node:child_process";
import { isDeepStrictEqual } from "node:util";

import { WORKSPACES, owningWorkspace } from "./ci-workspaces.mjs";

const APP_RELEASE_FILES = [
  "CHANGELOG.md",
  "apps/desktop/package.json",
  "apps/desktop/src-tauri/Cargo.lock",
  "apps/desktop/src-tauri/Cargo.toml",
  "apps/desktop/src-tauri/tauri.conf.json",
  // The remote helper names its release archive from this manifest and reports
  // the same version at run time, so a release bumps it with the application.
  "crates/antiburn-remote/Cargo.lock",
  "crates/antiburn-remote/Cargo.toml",
];

const ENGINE_RELEASE_FILES = [
  "apps/desktop/src-tauri/Cargo.lock",
  "crates/antiburn-local/CHANGELOG.md",
  "crates/antiburn-local/Cargo.lock",
  "crates/antiburn-local/Cargo.toml",
];

function sameFiles(actual, expected) {
  return (
    actual.length === expected.length &&
    [...actual]
      .sort()
      .every((file, index) => file === [...expected].sort()[index])
  );
}

function jsonWithVersionPlaceholder(contents) {
  const value = JSON.parse(contents);
  const version = value.version;
  if (typeof version !== "string") return null;
  value.version = "<version>";
  return { version, value };
}

function cargoTomlWithVersionPlaceholder(contents) {
  let inPackage = false;
  let version = null;
  const lines = contents.split(/(\r?\n)/);
  for (let index = 0; index < lines.length; index += 2) {
    const line = lines[index];
    const trimmed = line.trim();
    if (trimmed.startsWith("[")) {
      inPackage = trimmed === "[package]";
      continue;
    }
    if (!inPackage) continue;
    const match = /^(\s*version\s*=\s*")([^"]+)(".*)$/.exec(line);
    if (!match) continue;
    version = match[2];
    lines[index] = `${match[1]}<version>${match[3]}`;
    break;
  }
  return version === null ? null : { version, value: lines.join("") };
}

function cargoLockWithVersionPlaceholder(contents, packageName) {
  const found = cargoLockWithVersionsPlaceholder(contents, [packageName]);
  return found === null
    ? null
    : { version: found.versions[0], value: found.value };
}

/**
 * The same placeholder pass over several packages in one lockfile.
 *
 * The desktop lockfile records both `antiburn` and `antiburn-remote`, and a
 * release bumps them together. A pass over one of them alone sees the other's
 * new version as an unrelated change and rejects the release.
 *
 * Returns null unless every named package is present.
 */
function cargoLockWithVersionsPlaceholder(contents, packageNames) {
  const versions = [];
  const blocks = contents.split("[[package]]");
  for (const packageName of packageNames) {
    let version = null;
    for (let index = 1; index < blocks.length; index += 1) {
      const name = /^\s*name\s*=\s*"([^"]+)"/m.exec(blocks[index]);
      if (!name || name[1] !== packageName) continue;
      blocks[index] = blocks[index].replace(
        /^(\s*version\s*=\s*")([^"]+)(".*)$/m,
        (_whole, prefix, found, suffix) => {
          version = found;
          return `${prefix}<version>${suffix}`;
        },
      );
      break;
    }
    if (version === null) return null;
    versions.push(version);
  }
  return { versions, value: blocks.join("[[package]]") };
}

function changedOnlyVersion(before, after, normalizer) {
  const oldValue = normalizer(before);
  const newValue = normalizer(after);
  return (
    oldValue !== null &&
    newValue !== null &&
    oldValue.version !== newValue.version &&
    isDeepStrictEqual(oldValue.value, newValue.value)
  );
}

/** As changedOnlyVersion, but every named package must change. */
function changedOnlyVersions(before, after, packageNames) {
  const oldValue = cargoLockWithVersionsPlaceholder(before, packageNames);
  const newValue = cargoLockWithVersionsPlaceholder(after, packageNames);
  return (
    oldValue !== null &&
    newValue !== null &&
    oldValue.versions.every(
      (version, index) => version !== newValue.versions[index],
    ) &&
    isDeepStrictEqual(oldValue.value, newValue.value)
  );
}

export function isPureAppReleaseChange(files, readAtRef) {
  if (!sameFiles(files, APP_RELEASE_FILES)) return false;
  return (
    changedOnlyVersion(
      readAtRef("base", "apps/desktop/package.json"),
      readAtRef("head", "apps/desktop/package.json"),
      jsonWithVersionPlaceholder,
    ) &&
    changedOnlyVersion(
      readAtRef("base", "apps/desktop/src-tauri/tauri.conf.json"),
      readAtRef("head", "apps/desktop/src-tauri/tauri.conf.json"),
      jsonWithVersionPlaceholder,
    ) &&
    changedOnlyVersion(
      readAtRef("base", "apps/desktop/src-tauri/Cargo.toml"),
      readAtRef("head", "apps/desktop/src-tauri/Cargo.toml"),
      cargoTomlWithVersionPlaceholder,
    ) &&
    changedOnlyVersion(
      readAtRef("base", "crates/antiburn-remote/Cargo.toml"),
      readAtRef("head", "crates/antiburn-remote/Cargo.toml"),
      cargoTomlWithVersionPlaceholder,
    ) &&
    changedOnlyVersion(
      readAtRef("base", "crates/antiburn-remote/Cargo.lock"),
      readAtRef("head", "crates/antiburn-remote/Cargo.lock"),
      (contents) =>
        cargoLockWithVersionPlaceholder(contents, "antiburn-remote"),
    ) &&
    changedOnlyVersions(
      readAtRef("base", "apps/desktop/src-tauri/Cargo.lock"),
      readAtRef("head", "apps/desktop/src-tauri/Cargo.lock"),
      ["antiburn", "antiburn-remote"],
    )
  );
}

export function isPureEngineReleaseChange(files, readAtRef) {
  if (!sameFiles(files, ENGINE_RELEASE_FILES)) return false;
  return (
    changedOnlyVersion(
      readAtRef("base", "crates/antiburn-local/Cargo.toml"),
      readAtRef("head", "crates/antiburn-local/Cargo.toml"),
      cargoTomlWithVersionPlaceholder,
    ) &&
    changedOnlyVersion(
      readAtRef("base", "crates/antiburn-local/Cargo.lock"),
      readAtRef("head", "crates/antiburn-local/Cargo.lock"),
      (contents) => cargoLockWithVersionPlaceholder(contents, "antiburn-local"),
    ) &&
    changedOnlyVersion(
      readAtRef("base", "apps/desktop/src-tauri/Cargo.lock"),
      readAtRef("head", "apps/desktop/src-tauri/Cargo.lock"),
      (contents) => cargoLockWithVersionPlaceholder(contents, "antiburn-local"),
    )
  );
}

/**
 * Files outside every Cargo workspace that a workspace reads: at build time
 * with `include_str!` or `include_bytes!`, or at test time from a path
 * relative to CARGO_MANIFEST_DIR. A change to one of them must run the tests
 * of each listed workspace. Files inside another workspace need no entry:
 * they select that workspace, and its dependents follow. A test scans the
 * Rust sources and compares the result with this map.
 */
export const CROSS_WORKSPACE_INPUTS = {
  ".antiburn/checks/failed-retry-loop.yml": ["checks"],
  "apps/desktop/interface-scale.json": ["shell"],
  "apps/desktop/src/views/main-window/burn-checks/tests/fixtures/installed-check-report.json":
    ["shell"],
  "apps/desktop/src/views/SettingsView.tsx": ["shell"],
  "apps/desktop/src/views/settings/PrivacyPane.tsx": ["shell"],
  "docs/analytics.md": ["shell"],
  "docs/burn-check/examples/failed-retry-loop.yml": ["checks", "shell"],
  "docs/burn-check/examples/over-exploring.yml": ["checks", "shell"],
  "docs/burn-check/examples/scope-creep.yml": ["checks", "shell"],
  "docs/check-coverage.md": ["engine", "shell"],
  "docs/privacy-policy.md": ["shell"],
  "docs/session-coverage.md": ["engine"],
  "docs/support.md": ["engine", "shell"],
  "plugins/antiburn-live/hooks/catalog.ts": ["engine"],
};

/**
 * Root files that the desktop application ships. The frontend imports each
 * file with `?raw` (apps/desktop/src/lib/legalNotices.ts), and
 * apps/desktop/src-tauri/tauri.conf.json bundles each file as a resource. A
 * change runs the frontend and the shell. `licenses` also runs the license
 * checks, because they compare the file with the generated notices.
 */
export const BUNDLED_ROOT_FILES = {
  LICENSE: { licenses: false },
  NOTICE: { licenses: false },
  THIRD_PARTY_NOTICES: { licenses: true },
};

function crossWorkspaceReaders(file) {
  return Object.hasOwn(CROSS_WORKSPACE_INPUTS, file)
    ? CROSS_WORKSPACE_INPUTS[file]
    : [];
}

function isDocumentationPath(file) {
  return (
    file.startsWith("docs/") ||
    ["CHANGELOG.md", "CONTRIBUTING.md", "README.md", "SECURITY.md"].includes(
      file,
    )
  );
}

const PLATFORMS = {
  linux: { name: "linux", os: "ubuntu-24.04" },
  "linux-arm64": { name: "linux-arm64", os: "ubuntu-24.04-arm" },
  windows: { name: "windows", os: "windows-latest" },
  "windows-arm64": { name: "windows-arm64", os: "windows-11-arm" },
  macos: { name: "macos", os: "macos-latest" },
};

// Main and nightly use the same lists, so a new platform reaches both.
const ALL_RUST_PLATFORMS = Object.keys(PLATFORMS);
const ALL_FRONTEND_PLATFORMS = ["linux", "windows"];

const RUST_PLATFORMS = {
  draft: ["linux"],
  ready: ["linux", "windows", "macos"],
  main: ALL_RUST_PLATFORMS,
  nightly: ALL_RUST_PLATFORMS,
};

const FRONTEND_PLATFORMS = {
  draft: ["linux"],
  ready: ALL_FRONTEND_PLATFORMS,
  main: ALL_FRONTEND_PLATFORMS,
  nightly: ALL_FRONTEND_PLATFORMS,
};

const INSTALLER_FILES = [
  "install.sh",
  "install.ps1",
  "scripts/install-sh.test.mjs",
  "scripts/install-ps1.test.ps1",
  "scripts/windows-signing.ps1",
  "scripts/windows-signing.test.ps1",
  "scripts/setup-windows-signing.ps1",
];

const ROOT_NODE_FILES = [
  "package.json",
  "pnpm-lock.yaml",
  "pnpm-workspace.yaml",
];

// These scripts decide what CI runs or how it builds. Their own unit tests
// cannot show that every selected job still passes, so a change runs all jobs.
const FULL_RUN_SCRIPTS = [
  "scripts/ci-rust.mjs",
  "scripts/ci-workspaces.mjs",
  "scripts/classify-ci-changes.mjs",
  "scripts/package-remote-helper.sh",
];

const LICENSE_SCRIPT_FILES = ["scripts/third-party-notices.mjs"];

function isWorkspaceLicenseInput(name) {
  return [
    "Cargo.toml",
    "Cargo.lock",
    "deny.toml",
    "about.toml",
    "about.hbs",
  ].includes(name);
}

/**
 * License texts, notices, and SPDX files that the license checks read. The
 * match ignores case, so `License.md` and `copyright` also match, but
 * `licenses.rs` does not. Root files
 * in BUNDLED_ROOT_FILES must match too; a test checks this.
 */
export function isLegalFile(name) {
  return (
    /^(licen[cs]e|copying|copyright|notice|third_party_notices)([-._]|$)/i.test(
      name,
    ) || name.toLowerCase().endsWith(".spdx")
  );
}

// Nested crate directories inside a workspace. A file in one of them that
// the registry does not own belongs to a crate without a CI job.
const NESTED_CRATE_DIRS = ["crates/", "vendor/"];

function isUnregisteredCrate(file, workspace) {
  const inside = file.slice(workspace.directory.length + 1);
  return NESTED_CRATE_DIRS.some((dir) => inside.startsWith(dir));
}

/** `ids` plus every workspace that depends on one of them, transitively. */
export function withDependents(ids) {
  const selected = new Set(ids);
  let grew = true;
  while (grew) {
    grew = false;
    for (const workspace of WORKSPACES) {
      if (selected.has(workspace.id)) continue;
      if (workspace.dependsOn.some((id) => selected.has(id))) {
        selected.add(workspace.id);
        grew = true;
      }
    }
  }
  return WORKSPACES.map((workspace) => workspace.id).filter((id) =>
    selected.has(id),
  );
}

/**
 * Routes changed paths to the CI work they need. The result does not depend
 * on the stage; planOutputs applies the stage.
 */
export function routeChanges(files) {
  const route = {
    workspaces: [],
    frontend: false,
    ci_scripts: false,
    installers: false,
    licenses: false,
    validator: false,
    lockfile: false,
    full: false,
  };
  const direct = new Set();

  for (const file of files) {
    for (const id of crossWorkspaceReaders(file)) direct.add(id);
    if (file.startsWith("docs/burn-check/")) route.validator = true;
    if (isDocumentationPath(file)) continue;
    const name = file.slice(file.lastIndexOf("/") + 1);

    if (file.startsWith(".github/")) {
      route.full = true;
    } else if (ROOT_NODE_FILES.includes(file)) {
      route.frontend = true;
      route.ci_scripts = true;
      route.licenses = true;
    } else if (Object.hasOwn(BUNDLED_ROOT_FILES, file)) {
      direct.add("shell");
      route.frontend = true;
      if (BUNDLED_ROOT_FILES[file].licenses) route.licenses = true;
    } else if (INSTALLER_FILES.includes(file)) {
      route.installers = true;
    } else if (FULL_RUN_SCRIPTS.includes(file)) {
      route.full = true;
    } else if (
      LICENSE_SCRIPT_FILES.includes(file) ||
      file.startsWith("scripts/third-party-license-overrides/")
    ) {
      route.ci_scripts = true;
      route.licenses = true;
    } else if (
      file === "scripts/run-opencode-wsl-acceptance.ps1" ||
      file === "scripts/opencode-wsl-acceptance.test.mjs"
    ) {
      direct.add("engine");
      route.ci_scripts = true;
    } else if (file.startsWith("scripts/")) {
      route.ci_scripts = true;
    } else {
      // The workspace lookup comes before the frontend rule, because the
      // shell workspace is inside apps/desktop/.
      const workspace = owningWorkspace(file);
      if (workspace === null && file.startsWith("apps/desktop/")) {
        route.frontend = true;
        if (file === "apps/desktop/package.json") route.licenses = true;
        continue;
      }
      if (workspace === null && crossWorkspaceReaders(file).length > 0) {
        continue;
      }
      if (workspace === null || isUnregisteredCrate(file, workspace)) {
        route.full = true;
        continue;
      }
      direct.add(workspace.id);
      if (isWorkspaceLicenseInput(name) || isLegalFile(name)) {
        route.licenses = true;
      }
      if (name === "Cargo.lock") route.lockfile = true;
    }
  }

  if (route.full) {
    route.workspaces = WORKSPACES.map((workspace) => workspace.id);
    route.frontend = true;
    route.ci_scripts = true;
    route.installers = true;
    route.licenses = true;
    route.validator = true;
  } else {
    route.workspaces = withDependents([...direct]);
  }
  return route;
}

/** A route that selects every job, for nightly runs and failed diffs. */
export function routeEverything() {
  return routeChanges([".github/"]);
}

/** A route that selects nothing, for pure release bumps. */
function routeNothing() {
  return routeChanges([]);
}

export function resolveStage({ eventName, ref, draft, labels = [] }) {
  if (eventName === "pull_request") {
    return draft && !labels.includes("ci:full") ? "draft" : "ready";
  }
  if (eventName === "push") {
    return ref === "refs/heads/main" ? "main" : "ready";
  }
  // schedule, workflow_dispatch, and any event this script does not know.
  return "nightly";
}

function platformList(names) {
  return JSON.stringify(names.map((name) => PLATFORMS[name]));
}

function groupIds(workspaces, group) {
  return WORKSPACES.filter(
    (workspace) =>
      workspace.group === group && workspaces.includes(workspace.id),
  ).map((workspace) => workspace.id);
}

/**
 * The desktop job matrix: the shell in one part, the other desktop crates in
 * a second part, so that the two parts build in parallel. Empty parts drop.
 * `linux_deps` tells a Linux job to install the Tauri system packages.
 */
function desktopParts(desktopIds) {
  const parts = [
    { name: "shell", ids: desktopIds.filter((id) => id === "shell") },
    { name: "crates", ids: desktopIds.filter((id) => id !== "shell") },
  ];
  return JSON.stringify(
    parts
      .filter((part) => part.ids.length > 0)
      .map((part) => ({
        name: part.name,
        workspaces: part.ids.join(","),
        linux_deps: WORKSPACES.some(
          (workspace) =>
            part.ids.includes(workspace.id) &&
            workspace.linuxSystemDeps === true,
        ),
      })),
  );
}

/**
 * The exact GITHUB_OUTPUT map. `release` is { app, engine }; a release bump
 * overrides `changes` and selects only the release metadata jobs.
 */
export function planOutputs({ stage, changes, release = {} }) {
  const releaseApp = stage !== "nightly" && Boolean(release.app);
  const releaseEngine = stage !== "nightly" && Boolean(release.engine);
  let route = changes;
  if (stage === "nightly") route = routeEverything();
  else if (releaseApp || releaseEngine) route = routeNothing();

  const nightly = stage === "nightly";
  const draft = stage === "draft";
  const engineIds = groupIds(route.workspaces, "engine");
  const desktopIds = groupIds(route.workspaces, "desktop");
  const bool = (value) => (value ? "true" : "false");

  // A nightly route selects everything, so the run_* flags read the route
  // only. The stage limits drafts and adds the main-only and nightly-only jobs.
  return {
    release_app: bool(releaseApp),
    release_engine: bool(releaseEngine),
    rust_platforms: platformList(RUST_PLATFORMS[stage]),
    frontend_platforms: platformList(FRONTEND_PLATFORMS[stage]),
    engine_workspaces: engineIds.join(","),
    desktop_parts: desktopParts(desktopIds),
    run_engine: bool(engineIds.length > 0),
    run_desktop: bool(desktopIds.length > 0),
    run_frontend: bool(route.frontend),
    run_ci_scripts: bool(route.ci_scripts),
    run_validator: bool(route.validator),
    run_installers: bool(route.installers && !draft),
    run_licenses: bool(route.licenses && !draft),
    // Advisories change without a code change, so a pull request checks them
    // only when it changes a lockfile. Then a new advisory cannot fail an
    // unrelated pull request. Nightly and release runs check every time.
    run_advisories: bool(nightly || (!draft && route.lockfile)),
    // An engine change can break the static musl link of the helper, so
    // every stage after draft builds it when the remote workspace is selected.
    run_remote_build: bool(!draft && engineIds.includes("remote")),
    run_no_analytics_check: bool(
      (stage === "main" || nightly) && desktopIds.includes("shell"),
    ),
    run_release_metadata: bool(releaseApp || releaseEngine),
    run_warm_release_cache: bool(stage === "main" && releaseApp),
  };
}

/** Release bump detection. Returns { app, engine }. */
export function detectRelease(files, readAtRef) {
  if (isPureAppReleaseChange(files, readAtRef))
    return { app: true, engine: false };
  if (isPureEngineReleaseChange(files, readAtRef))
    return { app: false, engine: true };
  return { app: false, engine: false };
}

function git(...args) {
  return execFileSync("git", args, {
    encoding: "utf8",
    maxBuffer: 16 * 1024 * 1024,
  });
}

export function listChangedFiles(base, head, runGit = git) {
  // Do not filter statuses here. A deletion can remove executable code or a
  // dependency boundary just as surely as an addition can introduce one.
  return runGit("diff", "--name-only", base, head)
    .split(/\r?\n/)
    .filter(Boolean);
}

function readEventDraft(eventPath) {
  if (!eventPath || !existsSync(eventPath)) return false;
  const event = JSON.parse(readFileSync(eventPath, "utf8"));
  return event.pull_request?.draft === true;
}

/**
 * The live draft state wins over the event payload. A push and "ready for
 * review" can arrive together; the run that GitHub keeps can then carry a
 * payload that still says draft.
 */
export function resolveDraft(liveValue, readEvent) {
  if (liveValue === "true") return true;
  if (liveValue === "false") return false;
  return readEvent();
}

function parseLabels(value) {
  return (value ?? "")
    .split(/\r?\n/)
    .map((label) => label.trim())
    .filter(Boolean);
}

function isMissingSha(sha) {
  return !sha || /^0+$/.test(sha);
}

/** Changes and release state for the diff, or null to select everything. */
function inspectDiff(stage, base, head) {
  if (stage === "nightly") {
    console.log("Nightly run: ignore the diff and select every job.");
    return null;
  }
  if (isMissingSha(base) || isMissingSha(head)) {
    console.log(
      `No usable base or head SHA (${base || "empty"}..${head || "empty"}): select every job.`,
    );
    return null;
  }

  let files;
  try {
    files = listChangedFiles(base, head);
  } catch (error) {
    console.error(
      `::error::Could not classify ${base}..${head}: ${error.message}`,
    );
    process.exit(1);
  }
  console.log(
    `Changed files (${files.length}):\n${files.map((file) => `  ${file}`).join("\n")}`,
  );

  const readAtRef = (ref, file) =>
    git("show", `${ref === "base" ? base : head}:${file}`);
  try {
    return {
      changes: routeChanges(files),
      release: detectRelease(files, readAtRef),
    };
  } catch (error) {
    console.warn(
      `::warning::Semantic release classification failed: ${error.message}`,
    );
    return null;
  }
}

function main() {
  const [base = "", head = ""] = process.argv.slice(2);
  const stage = resolveStage({
    eventName: process.env.GITHUB_EVENT_NAME,
    ref: process.env.GITHUB_REF,
    draft: resolveDraft(process.env.CI_PR_DRAFT, () =>
      readEventDraft(process.env.GITHUB_EVENT_PATH),
    ),
    labels: parseLabels(process.env.CI_PR_LABELS),
  });
  console.log(`Stage: ${stage}`);

  const inspected = inspectDiff(stage, base, head);
  const outputs = planOutputs({
    stage,
    changes: inspected?.changes ?? routeEverything(),
    release: inspected?.release,
  });

  const lines = `${Object.entries(outputs)
    .map(([key, value]) => `${key}=${value}`)
    .join("\n")}\n`;
  console.log(`Outputs:\n${lines}`);
  if (process.env.GITHUB_OUTPUT) {
    appendFileSync(process.env.GITHUB_OUTPUT, lines);
  }
}

if (
  process.argv[1] &&
  import.meta.url === new URL(`file://${process.argv[1]}`).href
) {
  main();
}
