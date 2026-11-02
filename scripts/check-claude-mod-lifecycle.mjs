#!/usr/bin/env node
import assert from "node:assert/strict";
import {
  cpSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath, pathToFileURL } from "node:url";
import { PUBLIC, checkTree, releaseFiles } from "./assemble-claude-mod.mjs";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const temporary = mkdtempSync(join(tmpdir(), "antiburn-plugin-lifecycle-"));
const profile = join(temporary, "profile");
const project = join(temporary, "project");
mkdirSync(profile);
mkdirSync(project);
const release = "antiburn@antiburn";
const dev = "antiburn-dev@antiburn-local-dev";
const candidateFiles = releaseFiles(root);
const candidateVersion = JSON.parse(
  candidateFiles.get(".claude-plugin/plugin.json"),
).version;

function run(...args) {
  const result = spawnSync("claude", args, {
    cwd: project,
    env: { ...process.env, CLAUDE_CONFIG_DIR: profile },
    encoding: "utf8",
    timeout: 30000,
  });
  if (result.error || result.status !== 0) {
    throw new Error(
      `claude ${args.join(" ")} failed: ${result.error?.message ?? result.stderr ?? result.stdout}`,
    );
  }
  return result.stdout.trim();
}

function installed() {
  const listing = JSON.parse(run("plugin", "list", "--json"));
  assert.ok(Array.isArray(listing), "Unexpected plugin list schema");
  return listing;
}
function record(id, scope = "user") {
  return installed().find((entry) => entry.id === id && entry.scope === scope);
}
function catalog(directory, name, plugins) {
  mkdirSync(join(directory, ".claude-plugin"), { recursive: true });
  writeFileSync(
    join(directory, ".claude-plugin/marketplace.json"),
    JSON.stringify(
      {
        name,
        owner: { name: "antiburn" },
        plugins,
      },
      null,
      2,
    ) + "\n",
  );
}
function git(directory, ...args) {
  const result = spawnSync("git", args, {
    cwd: directory,
    encoding: "utf8",
    timeout: 30000,
  });
  assert.equal(result.status, 0, result.stderr || result.error?.message);
  return result.stdout.trim();
}
function commit(directory, message) {
  git(directory, "add", ".");
  git(
    directory,
    "-c",
    "user.name=Test Fixture",
    "-c",
    "user.email=fixture@example.invalid",
    "commit",
    "-s",
    "-m",
    message,
  );
  return git(directory, "rev-parse", "HEAD");
}

try {
  assert.deepEqual(installed(), []);
  const localCatalog = join(temporary, "local-catalog");
  cpSync(join(root, PUBLIC), join(localCatalog, "plugins/antiburn"), {
    recursive: true,
  });
  catalog(localCatalog, "antiburn", [
    { name: "antiburn", source: "./plugins/antiburn" },
  ]);
  run("plugin", "marketplace", "add", localCatalog);
  run(
    "plugin",
    "marketplace",
    "add",
    join(root, ".agent-artifacts/claude-mod/dev"),
  );
  for (const scope of ["user", "project", "local"]) {
    const identities = [release, dev];
    for (const id of identities) {
      run("plugin", "install", id, "--scope", scope, "--json");
      assert.equal(record(id, scope)?.enabled, true);
    }
    for (const id of identities) {
      const other = identities.find((candidate) => candidate !== id);
      const retained = join(profile, "plugins/data", id, `${scope}.json`);
      const fixtureData = { fixture: scope, identity: id };
      const version = record(id, scope).version;
      mkdirSync(dirname(retained), { recursive: true });
      writeFileSync(retained, JSON.stringify(fixtureData));
      run("plugin", "disable", id, "--scope", scope);
      assert.equal(record(id, scope)?.enabled, false);
      assert.equal(record(other, scope)?.enabled, true);
      run("plugin", "enable", id, "--scope", scope);
      assert.equal(record(id, scope)?.enabled, true);
      run("plugin", "update", id, "--scope", scope);
      assert.equal(record(id, scope)?.version, version);
      assert.equal(record(other, scope)?.enabled, true);
      run("plugin", "uninstall", id, "--scope", scope, "--keep-data");
      assert.equal(record(id, scope), undefined);
      assert.equal(record(other, scope)?.enabled, true);
      assert.deepEqual(JSON.parse(readFileSync(retained)), fixtureData);
      run("plugin", "install", id, "--scope", scope, "--json");
      assert.equal(record(id, scope)?.enabled, true);
      assert.equal(record(id, scope)?.version, version);
      assert.equal(record(other, scope)?.enabled, true);
      assert.deepEqual(JSON.parse(readFileSync(retained)), fixtureData);
    }
    for (const id of identities) {
      run("plugin", "uninstall", id, "--scope", scope, "--keep-data");
      assert.equal(record(id, scope), undefined);
    }
  }
  assert.deepEqual(installed(), []);
  run("plugin", "marketplace", "remove", "antiburn");
  console.log(
    "Local lifecycle passed in user/project/local scopes: independent public/dev install, disable, enable, update, uninstall with retained data and reinstall.",
  );

  const fixture = join(temporary, "fixture.git");
  mkdirSync(fixture);
  cpSync(join(root, PUBLIC), fixture, { recursive: true });
  git(fixture, "init", "-b", "main");
  commit(fixture, "Initial disposable unified plugin");
  catalog(localCatalog, "antiburn", [
    {
      name: "antiburn",
      source: { source: "url", url: pathToFileURL(fixture).href },
    },
  ]);
  run("plugin", "marketplace", "add", localCatalog);
  run("plugin", "install", release, "--scope", "user", "--json");
  const initial = record(release);
  assert.equal(initial.version, candidateVersion);
  assert.ok(initial.installPath.startsWith(join(profile, "plugins/cache/")));
  checkTree(initial.installPath, candidateFiles);
  const manifestPath = join(fixture, ".claude-plugin/plugin.json");
  const manifest = JSON.parse(readFileSync(manifestPath));
  const parts = /^(\d+)\.(\d+)\.(\d+)$/.exec(candidateVersion);
  assert.ok(parts, "The candidate needs a numeric version");
  const updateVersion = `${parts[1]}.${parts[2]}.${BigInt(parts[3]) + 1n}`;
  writeFileSync(
    manifestPath,
    JSON.stringify({ ...manifest, version: updateVersion }),
  );
  writeFileSync(join(fixture, "README.md"), "Disposable update fixture\n");
  commit(fixture, "Update disposable unified plugin");
  checkTree(initial.installPath, candidateFiles);
  run("plugin", "marketplace", "update", "antiburn");
  run("plugin", "update", release, "--scope", "user");
  const updated = record(release);
  assert.equal(updated.version, updateVersion);
  assert.equal(
    readFileSync(join(updated.installPath, "README.md"), "utf8"),
    "Disposable update fixture\n",
  );
  run("plugin", "uninstall", release, "--scope", "user", "--keep-data");
  run("plugin", "install", release, "--scope", "user", "--json");
  assert.equal(record(release).version, updateVersion);
  run("plugin", "uninstall", release, "--scope", "user", "--keep-data");
  assert.deepEqual(installed(), []);
  console.log(
    `Cached git lifecycle passed: exact ${candidateVersion} bytes, cache isolation, ${updateVersion} update, removal and reinstall.`,
  );
  console.log(
    "No production identity migration or legacy recovery catalog is required: there are no existing users. Local transports do not prove hosted delivery or native rendering.",
  );
} finally {
  rmSync(temporary, { recursive: true, force: true });
}
