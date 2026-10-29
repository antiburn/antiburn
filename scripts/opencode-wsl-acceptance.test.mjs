import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { parse } from "yaml";

import {
  planOutputs,
  resolveStage,
  routeChanges,
} from "./classify-ci-changes.mjs";

const workflow = parse(
  readFileSync(new URL("../.github/workflows/ci.yml", import.meta.url), "utf8"),
);
const script = readFileSync(
  new URL("./run-opencode-wsl-acceptance.ps1", import.meta.url),
  "utf8",
);

test("the synthetic CLI permits only the exact read-only V1 metadata probe", () => {
  const fixture = fileURLToPath(
    new URL(
      "../crates/antiburn-local/tests/fixtures/opencode_wsl_acceptance/test_cli_trap.py",
      import.meta.url,
    ),
  );
  const result = spawnSync("python3", ["-B", fixture], {
    encoding: "utf8",
    timeout: 30_000,
  });
  assert.ifError(result.error);
  assert.equal(result.status, 0, result.stdout + result.stderr);
});

test("manual dispatch selects native platforms and the WSL engine workspace", () => {
  const output = planOutputs({
    stage: resolveStage({ eventName: "workflow_dispatch" }),
    changes: routeChanges([]),
  });
  assert.deepEqual(
    JSON.parse(output.rust_platforms).map((platform) => platform.name),
    ["linux", "linux-arm64", "windows", "windows-arm64", "macos"],
  );
  assert.ok(output.engine_workspaces.split(",").includes("engine"));
  assert.equal(output.run_engine, "true");
});

test("selected Windows x64 engine jobs must run real WSL acceptance", () => {
  const step = workflow.jobs.engine.steps.find(
    (step) => step.run === "./scripts/run-opencode-wsl-acceptance.ps1",
  );
  assert.ok(step);
  assert.equal(
    step.if,
    "matrix.name == 'windows' && contains(format(',{0},', needs.classify.outputs.engine_workspaces), ',engine,')",
  );
  assert.equal(step.shell, "pwsh");
  assert.equal(step["timeout-minutes"], 15);
  assert.notEqual(step["continue-on-error"], true);
  assert.ok(workflow.jobs["ci-required"].needs.includes("engine"));
});

test("WSL provisioning and contract changes select their real engine acceptance", () => {
  for (const file of [
    "scripts/run-opencode-wsl-acceptance.ps1",
    "scripts/opencode-wsl-acceptance.test.mjs",
    "crates/antiburn-local/tests/fixtures/opencode_wsl_acceptance/server.py",
    "crates/antiburn-local/tests/fixtures/opencode_wsl_acceptance/test_cli_trap.py",
  ]) {
    const changes = routeChanges([file]);
    const output = planOutputs({ stage: "ready", changes });
    assert.ok(changes.workspaces.includes("engine"));
    assert.equal(output.run_engine, "true");
    assert.equal(output.run_ci_scripts, "true");
    assert.ok(
      JSON.parse(output.rust_platforms).some(
        (platform) => platform.name === "windows",
      ),
    );
  }
});

test("WSL setup bootstraps CA certificates before the HTTPS package snapshot", () => {
  const match = script.match(/\$setup = @'\r?\n([\s\S]*?)\r?\n'@/);
  assert.ok(match);
  const setup = match[1].replaceAll("\r\n", "\n");
  const stages = setup.split(
    "apt-get install --yes --no-install-recommends ca-certificates=20240203\n",
  );
  assert.equal(stages.length, 2);
  const [bootstrap, snapshot] = stages;
  assert.match(
    bootstrap,
    /cat > \/etc\/apt\/sources\.list <<'SOURCES'\ndeb http:\/\/archive\.ubuntu\.com\/ubuntu noble main\nSOURCES/,
  );
  assert.match(
    bootstrap,
    /apt-get -o Acquire::Retries=2 -o APT::Update::Error-Mode=any update/,
  );
  assert.match(snapshot, /^cat > \/etc\/apt\/sources\.list <<'SOURCES'\n/);
  assert.deepEqual(
    snapshot.split("\n").filter((line) => line.startsWith("deb ")),
    ["noble", "noble-updates", "noble-security"].map(
      (suite) =>
        `deb [check-valid-until=no] https://snapshot.ubuntu.com/ubuntu/20261020T000000Z ${suite} main`,
    ),
  );
  assert.match(
    snapshot,
    /SOURCES\napt-get -o Acquire::Retries=2 -o APT::Update::Error-Mode=any update\napt-get install --yes --no-install-recommends curl=8\.5\.0-2ubuntu10\.16 python3=3\.12\.3-0ubuntu2\.1/,
  );
  assert.doesNotMatch(
    setup,
    /--allow-unauthenticated|--allow-insecure-repositories|trusted=yes|Verify-(?:Peer|Host).*false|AllowInsecureRepositories.*true|--insecure/,
  );
});

test("WSL setup runs only the named ignored test and removes its own distro", () => {
  assert.match(script, /RUNNER_ENVIRONMENT -ne 'github-hosted'/);
  assert.match(script, /'AntiburnAcceptance-' \+ \[guid\]::NewGuid\(\)/);
  assert.match(script, /wsl\.exe --import \$name .* --version 2/);
  assert.match(
    script,
    /--test opencode_wsl_acceptance --run-ignored ignored-only/,
  );
  assert.match(
    script,
    /test\(production_bridge_reads_registered_wsl_service_without_starting_opencode\)/,
  );
  assert.match(script, /finally \{[\s\S]*wsl\.exe --unregister \$name/);
  assert.match(script, /wsl\.exe --terminate \$name/);
  assert.match(script, /wsl\.exe --distribution \$name --exec id -un/);
  assert.match(script, /\$defaultUser -ne 'antiburn'/);
  assert.doesNotMatch(script, /--shutdown|--set-default|--run-ignored all/);
});
