import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { parse } from "yaml";

import { selectDependabotNoticePullRequest } from "./verify-dependabot-notice-run.mjs";

const generationPath = new URL(
  "../.github/workflows/dependabot-notice-generation.yml",
  import.meta.url,
);
const writePath = new URL(
  "../.github/workflows/dependabot-notice-write.yml",
  import.meta.url,
);
const generation = parse(readFileSync(generationPath, "utf8"));
const write = parse(readFileSync(writePath, "utf8"));

function flatten(value) {
  if (typeof value === "string") return [value];
  if (Array.isArray(value)) return value.flatMap(flatten);
  if (value && typeof value === "object") {
    return Object.values(value).flatMap(flatten);
  }
  return [];
}

function validPullRequest(overrides = {}) {
  return {
    number: 42,
    state: "open",
    user: { login: "dependabot[bot]" },
    head: {
      ref: "dependabot/cargo/example/sha2-0.11.0",
      sha: "a".repeat(40),
      repo: { full_name: "antiburn/antiburn" },
    },
    base: {
      ref: "main",
      repo: { full_name: "antiburn/antiburn" },
    },
    ...overrides,
  };
}

function validRun(overrides = {}) {
  return {
    repository: "antiburn/antiburn",
    headRepository: "antiburn/antiburn",
    headBranch: "dependabot/cargo/example/sha2-0.11.0",
    pullRequests: [validPullRequest()],
    ...overrides,
  };
}

test("notice generation runs only for same-repository Dependabot pull requests", () => {
  assert.deepEqual(generation.on.pull_request.types, [
    "opened",
    "reopened",
    "synchronize",
  ]);
  assert.equal(generation.permissions.contents, "read");

  const job = generation.jobs.generate;
  assert.match(job.if, /dependabot\[bot\]/);
  assert.match(job.if, /github\.repository == 'antiburn\/antiburn'/);
  assert.match(job.if, /head\.repo\.full_name == github\.repository/);
  assert.match(job.if, /startsWith\(.*'dependabot\/'\)/);
  assert.equal(job.permissions, undefined);
});

test("generation installs without lifecycle scripts and uploads only changed notices", () => {
  const steps = generation.jobs.generate.steps;
  const install = steps.find((step) =>
    step.name.startsWith("Install frontend"),
  );
  const run = steps.find(
    (step) => step.name === "Generate third-party notices",
  );
  const upload = steps.find(
    (step) => step.name === "Upload the generated notice file",
  );

  assert.equal(install.run, "pnpm install --frozen-lockfile --ignore-scripts");
  assert.match(run.run, /pnpm notices/);
  assert.match(run.run, /git diff --quiet -- THIRD_PARTY_NOTICES/);
  assert.match(run.run, /echo "changed=false"/);
  assert.equal(
    upload.with.path,
    "${{ runner.temp }}/dependabot-notices/THIRD_PARTY_NOTICES",
  );
  assert.match(upload.if, /steps\.notice\.outputs\.changed == 'true'/);
  assert.doesNotMatch(
    flatten(generation.jobs.generate).join("\n"),
    /secrets\./,
  );
});

test("write workflow trusts only successful same-repository generation runs", () => {
  assert.deepEqual(write.on.workflow_run.workflows, [
    "Dependabot notice generation",
  ]);
  assert.deepEqual(write.on.workflow_run.types, ["completed"]);
  assert.equal(write.permissions.contents, "read");

  const prepare = write.jobs.prepare;
  assert.match(prepare.if, /workflow_run\.event == 'pull_request'/);
  assert.match(prepare.if, /workflow_run\.conclusion == 'success'/);
  assert.match(prepare.if, /head_repository\.full_name == github\.repository/);
  assert.match(prepare.if, /startsWith\(.*'dependabot\/'\)/);
  assert.equal(prepare.permissions.actions, "read");
  assert.equal(prepare.permissions["pull-requests"], "read");
  assert.equal(write.jobs.write.permissions.contents, "read");
  assert.match(
    prepare.steps.find((step) => step.name.startsWith("Find the exact")).run,
    /gh api -X GET/,
  );
});

test("the PAT appears only in the final guarded push step", () => {
  const steps = write.jobs.write.steps;
  const secretSteps = steps.filter((step) =>
    flatten(step).some((value) =>
      value.includes("secrets.DEPENDABOT_NOTICES_TOKEN"),
    ),
  );
  const push = steps.at(-1);
  const commit = steps.find(
    (step) =>
      step.name ===
      "Validate the current pull request and prepare a signed commit",
  );

  assert.equal(secretSteps.length, 1);
  assert.equal(push.name, "Push the signed notice commit with a branch lease");
  assert.match(push.if, /steps\.commit\.outputs\.changed == 'true'/);
  assert.equal(push.env.GH_TOKEN, "${{ secrets.DEPENDABOT_NOTICES_TOKEN }}");
  assert.match(push.run, /--force-with-lease/);
  assert.match(push.run, /HEAD:refs\/heads/);
  assert.match(commit.run, /git commit -s/);
  assert.match(commit.run, /HEAD_SHA/);
  assert.match(commit.run, /git diff --quiet -- THIRD_PARTY_NOTICES/);
  assert.match(commit.run, /echo "changed=false"/);
  assert.equal(steps[0].with["persist-credentials"], false);
  assert.doesNotMatch(flatten(steps).join("\n"), /pnpm install|pnpm notices/);
  assert.doesNotMatch(flatten(steps.slice(0, -1)).join("\n"), /secrets\./);
  assert.doesNotMatch(flatten(steps).join("\n"), /pull_request_target/);
});

test("run validation rejects an unrelated, stale, closed, or ambiguous pull request", () => {
  const valid = validRun();
  assert.deepEqual(selectDependabotNoticePullRequest(valid), {
    number: 42,
    head_ref: valid.headBranch,
    head_sha: valid.pullRequests[0].head.sha,
  });

  for (const invalidRun of [
    validRun({ repository: "attacker/repository" }),
    validRun({ headRepository: "attacker/repository" }),
    validRun({ headBranch: "user/branch" }),
  ]) {
    assert.throws(() => selectDependabotNoticePullRequest(invalidRun));
  }

  assert.equal(
    selectDependabotNoticePullRequest(
      validRun({ pullRequests: [validPullRequest({ state: "closed" })] }),
    ),
    null,
  );
  assert.equal(
    selectDependabotNoticePullRequest(
      validRun({
        pullRequests: [validPullRequest({ user: { login: "user" } })],
      }),
    ),
    null,
  );
  assert.equal(
    selectDependabotNoticePullRequest(
      validRun({
        pullRequests: [
          validPullRequest({
            head: { ...valid.pullRequests[0].head, sha: "invalid" },
          }),
        ],
      }),
    ),
    null,
  );
  assert.throws(() =>
    selectDependabotNoticePullRequest(
      validRun({
        pullRequests: [validPullRequest(), validPullRequest({ number: 43 })],
      }),
    ),
  );
});
