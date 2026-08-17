#!/usr/bin/env node
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { appendFileSync } from "node:fs";

export function selectTrustedMainCi(runs, sha) {
  const candidates = runs
    .filter(
      (run) =>
        run.head_sha === sha &&
        run.head_branch === "main" &&
        run.event === "push",
    )
    .sort((left, right) => (right.run_attempt ?? 0) - (left.run_attempt ?? 0));
  const success = candidates.find(
    (run) => run.status === "completed" && run.conclusion === "success",
  );
  if (success) return { state: "success", run: success };
  const waiting = candidates.find((run) => run.status !== "completed");
  if (waiting) return { state: "waiting", run: waiting };
  const failed = candidates.find((run) => run.status === "completed");
  if (failed) return { state: "failed", run: failed };
  return { state: "missing", run: null };
}

function sleep(milliseconds) {
  return new Promise((resolve) => setTimeout(resolve, milliseconds));
}

export function parsePositiveInteger(value, name) {
  if (!/^[1-9]\d*$/.test(value)) {
    throw new Error(`${name} must be a positive integer, got ${value}`);
  }
  const parsed = Number(value);
  if (!Number.isSafeInteger(parsed)) {
    throw new Error(`${name} is too large, got ${value}`);
  }
  return parsed;
}

export function requestTimeoutMilliseconds(deadline, now = Date.now()) {
  return Math.max(1, Math.min(30_000, deadline - now));
}

async function main() {
  const [repository, sha, workflow = "ci.yml"] = process.argv.slice(2);
  if (!repository || !sha) {
    console.error(
      "usage: wait-for-main-ci.mjs <owner/repo> <sha> [workflow-file]",
    );
    process.exit(2);
  }
  const token = process.env.GITHUB_TOKEN;
  if (!token) {
    console.error(
      "::error::GITHUB_TOKEN is required to inspect workflow runs.",
    );
    process.exit(2);
  }

  const api = process.env.GITHUB_API_URL ?? "https://api.github.com";
  let timeoutSeconds;
  let pollSeconds;
  try {
    timeoutSeconds = parsePositiveInteger(
      process.env.CI_WAIT_TIMEOUT_SECONDS ?? "1800",
      "CI_WAIT_TIMEOUT_SECONDS",
    );
    pollSeconds = parsePositiveInteger(
      process.env.CI_WAIT_POLL_SECONDS ?? "15",
      "CI_WAIT_POLL_SECONDS",
    );
  } catch (error) {
    console.error(`::error::${error.message}`);
    process.exit(2);
  }
  const deadline = Date.now() + timeoutSeconds * 1000;
  const endpoint = new URL(
    `${api}/repos/${repository}/actions/workflows/${workflow}/runs`,
  );
  endpoint.searchParams.set("branch", "main");
  endpoint.searchParams.set("event", "push");
  endpoint.searchParams.set("head_sha", sha);
  endpoint.searchParams.set("per_page", "20");

  while (Date.now() <= deadline) {
    let payload;
    try {
      const response = await fetch(endpoint, {
        headers: {
          Accept: "application/vnd.github+json",
          Authorization: `Bearer ${token}`,
          "X-GitHub-Api-Version": "2022-11-28",
        },
        signal: AbortSignal.timeout(requestTimeoutMilliseconds(deadline)),
      });
      if (!response.ok) {
        throw new Error(`HTTP ${response.status} ${await response.text()}`);
      }
      payload = await response.json();
    } catch (error) {
      if (error.name === "AbortError" || error.name === "TimeoutError") {
        if (Date.now() >= deadline) break;
        console.warn(
          `::warning::GitHub workflow-run lookup timed out; retrying in ${pollSeconds}s.`,
        );
        await sleep(
          Math.min(pollSeconds * 1000, Math.max(0, deadline - Date.now())),
        );
        continue;
      }
      console.error(
        `::error::GitHub workflow-run lookup failed: ${error instanceof Error ? error.message : String(error)}`,
      );
      process.exit(1);
    }
    const selected = selectTrustedMainCi(payload.workflow_runs ?? [], sha);
    if (selected.state === "success") {
      console.log(`Trusted main CI: ${selected.run.html_url} (${sha})`);
      if (process.env.GITHUB_OUTPUT) {
        appendFileSync(
          process.env.GITHUB_OUTPUT,
          `run_id=${selected.run.id}\nrun_url=${selected.run.html_url}\nsha=${sha}\n`,
        );
      }
      return;
    }
    if (selected.state === "failed") {
      console.error(
        `::error::Main CI for ${sha} completed with ${selected.run.conclusion}: ${selected.run.html_url}`,
      );
      process.exit(1);
    }
    const detail = selected.run ? ` (${selected.run.html_url})` : "";
    console.log(
      `Main CI for ${sha} is ${selected.state}${detail}; checking again in ${pollSeconds}s.`,
    );
    await sleep(
      Math.min(pollSeconds * 1000, Math.max(0, deadline - Date.now())),
    );
  }

  console.error(
    `::error::No successful main CI run for ${sha} within ${timeoutSeconds}s.`,
  );
  process.exit(1);
}

if (
  process.argv[1] &&
  import.meta.url === new URL(`file://${process.argv[1]}`).href
) {
  await main();
}
