import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { parse } from "yaml";

const workflow = parse(
  readFileSync(
    new URL("../.github/workflows/release-app.yml", import.meta.url),
    "utf8",
  ),
);
const credentialCheck = workflow.jobs.build.steps.find(
  (step) => step.id === "signing",
).run;
const azureVariables = [
  "AZURE_CLIENT_ID",
  "AZURE_TENANT_ID",
  "AZURE_SUBSCRIPTION_ID",
  "AZURE_SIGNING_ENDPOINT",
  "AZURE_SIGNING_ACCOUNT_NAME",
  "AZURE_SIGNING_CERTIFICATE_PROFILE_NAME",
];

function checkCredentials(overrides) {
  const directory = mkdtempSync(join(tmpdir(), "windows-signing-test-"));
  const outputPath = join(directory, "output");
  try {
    const result = spawnSync("bash", ["-c", credentialCheck], {
      encoding: "utf8",
      env: {
        ...process.env,
        ...Object.fromEntries(azureVariables.map((name) => [name, ""])),
        RUNNER_OS: "Windows",
        UPDATER_KEY: "synthetic-updater-key",
        ALLOW_UNSIGNED_WINDOWS: "",
        GITHUB_OUTPUT: outputPath,
        ...overrides,
      },
    });
    assert.ifError(result.error);
    return {
      status: result.status,
      stdout: result.stdout,
      output: result.status === 0 ? readFileSync(outputPath, "utf8") : "",
    };
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
}

test("configured Azure signing takes precedence over the unsigned waiver", () => {
  const result = checkCredentials({
    ...Object.fromEntries(
      azureVariables.map((name) => [name, "synthetic-value"]),
    ),
    ALLOW_UNSIGNED_WINDOWS: "true",
  });
  assert.equal(result.status, 0);
  assert.equal(result.output, "platform_signing=authenticode\n");
});

for (const missing of azureVariables) {
  test(`missing ${missing} fails even with the unsigned waiver`, () => {
    const result = checkCredentials({
      ...Object.fromEntries(
        azureVariables.map((name) => [name, "synthetic-value"]),
      ),
      [missing]: "",
      ALLOW_UNSIGNED_WINDOWS: "true",
    });
    assert.notEqual(result.status, 0);
    assert.match(result.stdout, new RegExp(missing));
    assert.equal(result.output, "");
  });
}

test("an unconfigured Windows release fails without an explicit waiver", () => {
  assert.notEqual(checkCredentials({}).status, 0);
});

test("the legacy waiver applies only when Azure signing is entirely unconfigured", () => {
  const result = checkCredentials({ ALLOW_UNSIGNED_WINDOWS: "true" });
  assert.equal(result.status, 0);
  assert.equal(result.output, "platform_signing=unsigned\n");
});

test("Azure signing cannot waive the updater key", () => {
  const result = checkCredentials({
    ...Object.fromEntries(
      azureVariables.map((name) => [name, "synthetic-value"]),
    ),
    UPDATER_KEY: "",
  });
  assert.notEqual(result.status, 0);
  assert.match(result.stdout, /TAURI_SIGNING_PRIVATE_KEY/);
});
