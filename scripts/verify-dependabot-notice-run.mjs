#!/usr/bin/env node
const REPOSITORY = "antiburn/antiburn";

export function selectDependabotNoticePullRequest({
  repository,
  headRepository,
  headBranch,
  pullRequests,
}) {
  if (
    repository !== REPOSITORY ||
    headRepository !== repository ||
    typeof headBranch !== "string" ||
    !headBranch.startsWith("dependabot/") ||
    !Array.isArray(pullRequests)
  ) {
    throw new Error("Workflow run is not an eligible Dependabot pull request");
  }

  const matches = pullRequests.filter(
    (pullRequest) =>
      pullRequest.state === "open" &&
      pullRequest.user?.login === "dependabot[bot]" &&
      pullRequest.head?.repo?.full_name === repository &&
      pullRequest.head.ref === headBranch &&
      typeof pullRequest.head.sha === "string" &&
      /^[0-9a-f]{40}$/i.test(pullRequest.head.sha) &&
      pullRequest.base?.repo?.full_name === repository &&
      pullRequest.base.ref === "main" &&
      Number.isSafeInteger(pullRequest.number) &&
      pullRequest.number > 0,
  );

  if (matches.length === 0) return null;
  if (matches.length !== 1) {
    throw new Error(
      `Expected one open Dependabot pull request for ${headBranch}; found ${matches.length}`,
    );
  }

  return {
    number: matches[0].number,
    head_ref: headBranch,
    head_sha: matches[0].head.sha,
  };
}

async function readStdin() {
  const chunks = [];
  for await (const chunk of process.stdin) chunks.push(chunk);
  return Buffer.concat(chunks).toString("utf8");
}

async function main() {
  try {
    const pullRequests = JSON.parse(await readStdin());
    const result = selectDependabotNoticePullRequest({
      repository: process.env.GITHUB_REPOSITORY,
      headRepository: process.env.HEAD_REPOSITORY,
      headBranch: process.env.HEAD_BRANCH,
      pullRequests,
    });
    process.stdout.write(`${JSON.stringify(result ?? { eligible: false })}\n`);
  } catch (error) {
    console.error(error.message);
    process.exitCode = 1;
  }
}

if (
  process.argv[1] &&
  import.meta.url === new URL(`file://${process.argv[1]}`).href
) {
  await main();
}
