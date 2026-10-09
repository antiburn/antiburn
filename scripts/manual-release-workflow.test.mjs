import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { spawnSync } from "node:child_process";
import test from "node:test";
import { parse } from "yaml";

const workflow = parse(
  readFileSync(
    new URL("../.github/workflows/release-app.yml", import.meta.url),
    "utf8",
  ),
);

test("manual builds use the complete desktop matrix and remote helpers", () => {
  assert.equal(workflow.on.workflow_dispatch.inputs.version.required, true);
  assert.equal(workflow.jobs.build.strategy.matrix.include.length, 6);
  assert.equal(
    workflow.jobs["remote-helper"].uses,
    "./.github/workflows/build-remote-helper.yml",
  );
  assert.deepEqual(workflow.jobs.assemble.needs, [
    "verify",
    "sbom",
    "build",
    "remote-helper",
  ]);
  for (const job of [
    workflow.jobs.verify,
    workflow.jobs.sbom,
    workflow.jobs.build,
  ]) {
    const preparation = job.steps.find(
      (step) => step.name === "Apply the manual build version",
    );
    assert.ok(preparation);
    assert.equal(preparation.if, "github.event_name == 'workflow_dispatch'");
    assert.equal(preparation.shell, "bash");
  }
});

test("only tag-triggered drafts have repository write access and GitHub Releases commands", () => {
  for (const [name, job] of Object.entries(workflow.jobs)) {
    if (job.permissions?.contents === "write") {
      assert.equal(name, "draft");
      assert.equal(job.if, "github.event_name == 'push'");
    }
    for (const step of job.steps ?? []) {
      if (/gh release (?:create|edit|upload)/.test(step.run ?? "")) {
        assert.equal(name, "draft");
      }
    }
  }
  assert.equal(workflow.jobs.assemble.permissions.contents, "read");
  const upload = workflow.jobs.assemble.steps.find(
    (step) => step.name === "Upload the assembled assets",
  );
  assert.equal(upload.with.name, "release");
});

test("manual builds reject branch and tag refs other than main", () => {
  const guard = workflow.jobs.verify.steps.find(
    (step) => step.name === "Require main for manual builds",
  ).run;
  for (const ref of [
    "refs/heads/main",
    "refs/heads/feature",
    "refs/tags/antiburn-v0.9.0",
  ]) {
    const result = spawnSync("bash", ["-c", guard], {
      env: { ...process.env, EVENT: "workflow_dispatch", REF: ref },
    });
    assert.ifError(result.error);
    assert.equal(result.status, ref === "refs/heads/main" ? 0 : 1);
  }
});

test("tag builds retain the engine source release gate", () => {
  const gate = workflow.jobs.verify.steps.find(
    (step) => step.run === "node scripts/verify-app-engine-release.mjs",
  );
  assert.equal(gate.if, "github.event_name == 'push'");
  assert.ok(workflow.jobs.build.needs.includes("trusted-main-ci"));
  assert.ok(workflow.jobs["remote-helper"].needs.includes("trusted-main-ci"));
});
