import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  mkdtemp,
  mkdir,
  readFile,
  realpath,
  readdir,
  rm,
  symlink,
  writeFile,
} from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

const root = resolve(import.meta.dirname, "..");
const script = fileURLToPath(
  new URL("./trigger-yaml-check-audit.mjs", import.meta.url),
);
const ids = [
  "over-exploring",
  "scope-creep",
  "ignored-instructions",
  "skill-opportunities",
  "fast-mode-value",
];
const minimums = [10, 5, 3, 3, 3];

async function fixture() {
  const directory = await mkdtemp(join(tmpdir(), "antiburn-yaml-audit-test-"));
  const repo = join(directory, "repo");
  await mkdir(repo);
  const git = spawnSync("git", ["init", "-q"], { cwd: repo });
  assert.equal(git.status, 0);
  await mkdir(join(repo, ".antiburn/checks"), { recursive: true });
  return { directory, repo };
}

async function installDefinition(repo, id) {
  const source = await readFile(join(root, ".antiburn/checks", `${id}.yml`));
  await writeFile(join(repo, ".antiburn/checks", `${id}.yml`), source);
  return source;
}

function run(args, env = process.env) {
  return spawnSync(process.execPath, [script, ...args], {
    encoding: "utf8",
    env,
  });
}

test("prepares fixture metadata for all five current canonical checks without evaluation claims", async (t) => {
  const { directory, repo } = await fixture();
  t.after(() => rm(directory, { recursive: true, force: true }));
  for (const [index, id] of ids.entries()) {
    const source = await installDefinition(repo, id);
    const output = join(directory, `out-${id}`);
    const result = run([
      "--cwd",
      repo,
      "--output",
      output,
      "--check",
      id,
      "--vendor",
      "claude",
    ]);
    assert.equal(result.status, 0, result.stderr);
    const manifest = JSON.parse(result.stdout);
    assert.equal(manifest.check_id, id);
    assert.equal(manifest.scenario, "finding");
    assert.equal(
      manifest.definition_sha256,
      createHash("sha256").update(source).digest("hex"),
    );
    assert.equal(manifest.scripted_results, minimums[index]);
    assert.equal(manifest.evaluation_verified, false);
    assert.equal(manifest.assessment_seeded, false);
    assert.equal(manifest.provider_response_seeded, false);
    assert.equal(manifest.installed, false);
    assert.equal(manifest.native_file, null);
    assert.equal(manifest.setup_files.length > 0, true);
    for (const setup of manifest.setup_files) {
      const content = await readFile(join(output, setup.output_file));
      assert.equal(
        setup.sha256,
        createHash("sha256").update(content).digest("hex"),
      );
      assert.equal(
        await readFile(join(repo, setup.path)).then(
          () => true,
          () => false,
        ),
        false,
      );
    }
    assert.equal(
      (await readFile(join(output, "scripted-session.jsonl"), "utf8"))
        .split("\n")
        .filter(Boolean).length,
      1 + 2 * manifest.scripted_results,
    );
  }
});

test("finding and control scenarios carry distinct evidence for every supported pair", async (t) => {
  const { directory, repo } = await fixture();
  t.after(() => rm(directory, { recursive: true, force: true }));
  for (const id of ids) await installDefinition(repo, id);
  for (const vendor of ["claude", "codex", "pi"]) {
    for (const id of ids) {
      const manifests = {};
      const sessions = {};
      for (const choice of ["finding", "control"]) {
        const output = join(directory, `out-${vendor}-${id}-${choice}`);
        const result = run([
          "--cwd",
          repo,
          "--output",
          output,
          "--check",
          id,
          "--vendor",
          vendor,
          "--scenario",
          choice,
        ]);
        assert.equal(result.status, 0, result.stderr);
        manifests[choice] = JSON.parse(result.stdout);
        sessions[choice] = (
          await readFile(join(output, "scripted-session.jsonl"), "utf8")
        )
          .split("\n")
          .filter(Boolean)
          .map((line) => JSON.parse(line));
        if (vendor === "codex") {
          assert.deepEqual(
            sessions[choice].map(({ ordinal }) => ordinal),
            sessions[choice].map((_, ordinal) => ordinal),
          );
          const turn = sessions[choice].find(
            ({ type }) => type === "turn_context",
          );
          assert.equal(turn.payload.turn_id, "turn-1");
          assert.equal(turn.payload.cwd, await realpath(repo));
          assert.equal(turn.payload.approval_policy, "never");
          assert.equal(turn.payload.approvals_reviewer, "user");
          assert.deepEqual(turn.payload.workspace_roots, [
            await realpath(repo),
          ]);
          assert.deepEqual(turn.payload.sandbox_policy, { type: "read-only" });
          assert.equal(turn.payload.permission_profile.type, "managed");
          assert.equal(turn.payload.permission_profile.network, "restricted");
          assert.deepEqual(turn.payload.collaboration_mode, {
            mode: "default",
            settings: {
              model: "scripted-audit-fixture",
              reasoning_effort: null,
              developer_instructions: null,
            },
          });
          const requests = sessions[choice].filter(
            ({ type, payload }) =>
              type === "response_item" && payload.role === "user",
          );
          assert.equal(requests.length, 1);
          for (const [index, row] of requests.entries()) {
            assert.equal(row.metadata.client_authored, false);
            assert.equal(row.metadata.user_input_order, index);
            assert.equal(row.metadata.retained_source.complete, true);
            assert.equal(
              row.metadata.retained_source.id.message_id,
              row.payload.id,
            );
            assert.equal(row.metadata.retained_source.id.turn_id, "turn-1");
            assert.equal(row.metadata.retained_source.id.role, "user");
            assert.deepEqual(
              row.payload.internal_chat_message_metadata_passthrough,
              { turn_id: "turn-1", content_item_kinds: ["user.text"] },
            );
          }
          if (id === "fast-mode-value") {
            const settings = sessions[choice].find(
              ({ payload }) => payload?.type === "thread_settings_applied",
            );
            assert.equal(
              settings.payload.thread_settings.model,
              turn.payload.model,
            );
            assert.equal(
              settings.payload.thread_settings.service_tier,
              "priority",
            );
            assert.equal(
              settings.payload.thread_settings.approval_policy,
              "never",
            );
            assert.equal(
              settings.payload.thread_settings.approvals_reviewer,
              "user",
            );
            assert.deepEqual(
              settings.payload.thread_settings.runtime_workspace_roots,
              [await realpath(repo)],
            );
            assert.deepEqual(
              settings.payload.thread_settings.disabled_plugin_ids,
              [],
            );
            assert.deepEqual(
              settings.payload.thread_settings.collaboration_mode,
              turn.payload.collaboration_mode,
            );
          }
          for (const row of sessions[choice].filter(
            ({ payload }) => payload?.type === "item_completed",
          )) {
            assert.equal(row.payload.thread_id, manifests[choice].session_id);
            assert.equal(row.payload.turn_id, "turn-1");
          }
          if (id === "skill-opportunities" && choice === "finding") {
            const failure = sessions[choice].find(
              ({ payload }) =>
                payload?.type === "item_completed" &&
                payload.item?.type === "CommandExecution" &&
                payload.item.exit_code === 101,
            );
            assert.equal(failure.payload.item.exit_code, 101);
            assert.equal(failure.payload.item.status, "failed");
            assert.match(failure.payload.item.stderr, /couldn't read/);
            assert.equal(failure.payload.item.stdout, "");
          }
        }
        if (
          vendor === "claude" &&
          id === "skill-opportunities" &&
          choice === "finding"
        ) {
          const failure = sessions[choice].find(
            ({ type, message }) =>
              type === "user" &&
              message.content?.[0]?.type === "tool_result" &&
              message.content[0].is_error === true,
          );
          assert.equal(failure.message.content[0].is_error, true);
          assert.match(failure.message.content[0].content, /couldn't read/);
        }
        if (
          vendor === "pi" &&
          id === "skill-opportunities" &&
          choice === "finding"
        ) {
          const failure = sessions[choice].find(
            ({ message }) =>
              message?.role === "toolResult" &&
              message.content?.[0]?.text?.includes("couldn't read"),
          );
          assert.equal(failure.message.isError, true);
          assert.match(failure.message.content[0].text, /couldn't read/);
        }
        if (vendor === "pi") {
          const calls = sessions[choice].flatMap((row) =>
            (row.message?.content ?? []).filter(
              (item) => item.type === "toolCall",
            ),
          );
          const edits = calls.filter(({ name }) => name === "edit");
          for (const call of calls.filter(({ name }) =>
            ["read", "edit"].includes(name),
          ))
            assert.ok(
              call.arguments.path.startsWith(await realpath(repo)),
              `${id}/${choice} Pi ${call.name} path must be absolute`,
            );
          const expectedEdits =
            id === "scope-creep" ? 5 : id === "ignored-instructions" ? 1 : 0;
          assert.equal(edits.length, expectedEdits, `${id}/${choice}`);
          for (const call of edits) {
            assert.ok(Array.isArray(call.arguments.edits), `${id}/${choice}`);
            assert.ok(call.arguments.edits.length > 0, `${id}/${choice}`);
            for (const edit of call.arguments.edits) {
              assert.equal(typeof edit.oldText, "string");
              assert.equal(typeof edit.newText, "string");
            }
            assert.equal("old" in call.arguments, false);
            assert.equal("new" in call.arguments, false);
          }
        }
        assert.equal(manifests[choice].scenario, choice);
        assert.equal(manifests[choice].vendor, vendor);
        assert.equal(manifests[choice].check_id, id);
        assert.equal(
          manifests[choice].limits.includes(
            "does not guarantee an evaluation label",
          ),
          true,
        );
      }
      assert.notDeepEqual(
        sessions.finding,
        sessions.control,
        `${vendor}/${id}`,
      );
      if (id === "fast-mode-value") {
        assert.ok(
          JSON.stringify(sessions.finding).includes(
            "not urgent; slower work is acceptable",
          ),
        );
        assert.ok(
          JSON.stringify(sessions.control).includes("low latency is required"),
        );
      }
      if (id === "ignored-instructions") {
        const findingText = JSON.stringify(sessions.finding);
        const controlText = JSON.stringify(sessions.control);
        assert.match(findingText, /AUDIT_FORBIDDEN_MARKER/);
        assert.doesNotMatch(controlText, /AUDIT_FORBIDDEN_MARKER/);
        assert.doesNotMatch(findingText, /CLAUDE\.md|AGENTS\.md/);
      }
      if (id === "skill-opportunities") {
        const skill = manifests.finding.setup_files.find(({ path }) =>
          path.endsWith("SKILL.md"),
        );
        assert.ok(skill);
        assert.equal(
          skill.sha256,
          manifests.control.setup_files.find(({ path }) => path === skill.path)
            .sha256,
        );
      }
    }
  }
});

test("finding and control evidence encodes concrete, scoped scenarios", async (t) => {
  const { directory, repo } = await fixture();
  t.after(() => rm(directory, { recursive: true, force: true }));
  for (const id of ids) await installDefinition(repo, id);

  let generated = 0;
  const generate = async (id, choice, vendor = "codex") => {
    const output = join(directory, `${id}-${choice}-${vendor}-${generated++}`);
    const result = run([
      "--cwd",
      repo,
      "--output",
      output,
      "--check",
      id,
      "--vendor",
      vendor,
      "--scenario",
      choice,
    ]);
    assert.equal(result.status, 0, result.stderr);
    const manifest = JSON.parse(result.stdout);
    const inputs = new Map(
      await Promise.all(
        manifest.setup_files.map(async ({ path, output_file }) => [
          path,
          await readFile(join(output, output_file), "utf8"),
        ]),
      ),
    );
    const session = (
      await readFile(join(output, "scripted-session.jsonl"), "utf8")
    )
      .split("\n")
      .filter(Boolean)
      .map((line) => JSON.parse(line));
    return { inputs, session };
  };

  for (const choice of ["finding", "control"]) {
    const { inputs } = await generate("over-exploring", choice);
    const parser = inputs.get("src/parser.rs");
    assert.match(parser, /None => Ok\(String::new\(\)\)/);
    assert.match(parser, /Some\(value\) if value\.is_empty\(\) => Err/);
    if (choice === "finding") {
      const research = [...inputs].filter(([path]) =>
        path.startsWith("audit-fixtures/account-dashboard-research-"),
      );
      assert.equal(research.length, minimums[0]);
      assert.ok(research.every(([, content]) => content.length > 500));
      assert.ok(
        research.every(([, content]) =>
          /separate account dashboard/.test(content),
        ),
      );
    } else {
      assert.equal(inputs.size, 1);
    }
  }

  const scope = await generate("scope-creep", "finding");
  const changedCode = [...scope.session]
    .map((row) => JSON.stringify(row))
    .join("\n");
  assert.match(changedCode, /AccountDashboard/);
  assert.match(changedCode, /export_csv/);
  assert.match(changedCode, /dashboard_filters_and_exports_accounts/);

  const ignored = await generate("ignored-instructions", "finding");
  assert.match(
    ignored.inputs.get("AGENTS.md"),
    /Do not add the literal AUDIT_FORBIDDEN_MARKER/,
  );
  const ignoredActivities = JSON.stringify(ignored.session);
  assert.match(ignoredActivities, /AUDIT_FORBIDDEN_MARKER/);
  assert.doesNotMatch(ignoredActivities, /audit-correction-/);
  assert.doesNotMatch(ignoredActivities, /AGENTS\.md|CLAUDE\.md/);
  const parserReadIndex = ignoredActivities.indexOf(
    "None => Ok(String::new())",
  );
  const editIndex = ignoredActivities.indexOf("AUDIT_FORBIDDEN_MARKER");
  const proofIndex = ignoredActivities.lastIndexOf("AUDIT_FORBIDDEN_MARKER");
  assert.ok(parserReadIndex >= 0 && parserReadIndex < editIndex);
  assert.ok(editIndex < proofIndex);

  const skill = await generate("skill-opportunities", "finding", "claude");
  assert.match(
    skill.inputs.get(".claude/skills/parser-codegen/SKILL.md"),
    /cargo run --offline --manifest-path tools\/parser-codegen\/Cargo\.toml -- grammar\/parser\.peg src\/generated\/parser_tables\.rs/,
  );
  assert.match(skill.inputs.get("Cargo.toml"), /name = "parser-fixture"/);
  assert.match(
    skill.inputs.get("tools/parser-codegen/Cargo.toml"),
    /name = "parser-codegen"/,
  );
  assert.match(skill.inputs.get("grammar/parser.peg"), /Input <- Spacing/);
  assert.match(
    skill.inputs.get("tools/parser-codegen/src/main.rs"),
    /create_dir_all/,
  );
  assert.match(
    JSON.stringify(skill.session),
    /couldn't read src\/generated\/parser_tables\.rs/,
  );
  const skillFindingCalls = skill.session.flatMap((row) =>
    (Array.isArray(row.message?.content) ? row.message.content : []).filter(
      (item) => item.type === "tool_use",
    ),
  );
  assert.ok(
    skillFindingCalls.some(
      ({ name, input }) =>
        name === "Read" && input.file_path.endsWith("grammar/parser.peg"),
    ),
  );
  assert.equal(
    skillFindingCalls.some(
      ({ name, input }) =>
        name === "Read" && input.file_path.endsWith("parser_tables.rs"),
    ),
    false,
  );
  assert.doesNotMatch(
    JSON.stringify(skill.session),
    /cargo run --offline --manifest-path tools\/parser-codegen\/Cargo\.toml/,
  );

  const skillControl = await generate(
    "skill-opportunities",
    "control",
    "claude",
  );
  const skillControlEvidence = JSON.stringify(skillControl.session);
  assert.match(
    skillControlEvidence,
    /cargo run --offline --manifest-path tools\/parser-codegen\/Cargo\.toml/,
  );
  assert.match(skillControlEvidence, /src\/generated\/parser_tables\.rs/);
  assert.match(skillControlEvidence, /PARSER_GRAMMAR/);
  assert.match(
    skillControlEvidence,
    /test parser::tests::parses_nonempty_input \.\.\. ok/,
  );
  const skillControlCalls = skillControl.session.flatMap((row) =>
    (Array.isArray(row.message?.content) ? row.message.content : []).filter(
      (item) => item.type === "tool_use",
    ),
  );
  const generationIndex = skillControlCalls.findIndex(
    ({ name, input }) =>
      name === "Bash" &&
      input.command.startsWith(
        "cargo run --offline --manifest-path tools/parser-codegen/Cargo.toml",
      ),
  );
  const generatedReadIndex = skillControlCalls.findIndex(
    ({ name, input }) =>
      name === "Read" && input.file_path.endsWith("parser_tables.rs"),
  );
  const passingTestIndex = skillControlCalls.findIndex(
    ({ name, input }) =>
      name === "Bash" && input.command === "cargo test parser",
  );
  assert.ok(generationIndex >= 0 && generationIndex < generatedReadIndex);
  assert.ok(generatedReadIndex < passingTestIndex);

  for (const vendor of ["claude", "codex", "pi"]) {
    const ignoredVendor = await generate(
      "ignored-instructions",
      "finding",
      vendor,
    );
    const skillVendor = await generate(
      "skill-opportunities",
      "finding",
      vendor,
    );
    if (vendor === "claude") {
      assert.ok(ignoredVendor.inputs.has("CLAUDE.md"));
      assert.equal(ignoredVendor.inputs.has("AGENTS.md"), false);
      assert.ok(skillVendor.inputs.has("CLAUDE.md"));
      assert.ok(
        skillVendor.inputs.has(".claude/skills/parser-codegen/SKILL.md"),
      );
      assert.equal(
        skillVendor.inputs.has(".agents/skills/parser-test-review/SKILL.md"),
        false,
      );
    } else {
      assert.ok(ignoredVendor.inputs.has("AGENTS.md"));
      assert.equal(ignoredVendor.inputs.has("CLAUDE.md"), false);
      assert.ok(
        skillVendor.inputs.has(".agents/skills/parser-test-review/SKILL.md"),
      );
      assert.equal(skillVendor.inputs.has("CLAUDE.md"), false);
      assert.equal(
        skillVendor.inputs.has(".claude/skills/parser-codegen/SKILL.md"),
        false,
      );
    }
  }
});

test("requires a current unchanged canonical definition and refuses existing output", async (t) => {
  const { directory, repo } = await fixture();
  t.after(() => rm(directory, { recursive: true, force: true }));
  const output = join(directory, "audit");
  let result = run([
    "--cwd",
    repo,
    "--output",
    output,
    "--check",
    "scope-creep",
  ]);
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /Install the unchanged scope-creep definition/);
  await installDefinition(repo, "scope-creep");
  result = run(["--cwd", repo, "--output", output, "--check", "scope-creep"]);
  assert.equal(result.status, 0, result.stderr);
  result = run(["--cwd", repo, "--output", output, "--check", "scope-creep"]);
  assert.notEqual(result.status, 0);
  await writeFile(join(repo, ".antiburn/checks/scope-creep.yml"), "changed\n");
  result = run([
    "--cwd",
    repo,
    "--output",
    join(directory, "changed"),
    "--check",
    "scope-creep",
  ]);
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /unchanged scope-creep/);
});

test("rejects removed IDs and unsupported vendors", async (t) => {
  const { directory, repo } = await fixture();
  t.after(() => rm(directory, { recursive: true, force: true }));
  for (const [id, vendor, pattern] of [
    ["failed-retry-loop", "claude", /Unsupported canonical check/],
    ["over-exploring", "cursor", /Use --vendor/],
  ]) {
    if (ids.includes(id)) await installDefinition(repo, id);
    const result = run([
      "--cwd",
      repo,
      "--output",
      join(directory, `out-${id}-${vendor}`),
      "--check",
      id,
      "--vendor",
      vendor,
    ]);
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, pattern);
  }
});

test("installs only into an explicit isolated native directory and never overwrites a session", async (t) => {
  const { directory, repo } = await fixture();
  t.after(() => rm(directory, { recursive: true, force: true }));
  await installDefinition(repo, "ignored-instructions");
  const native = join(directory, "audit", "claude-profile", "projects");
  const ambientProfile = join(directory, "ambient-claude-profile");
  const env = { ...process.env, CLAUDE_CONFIG_DIR: ambientProfile };
  const result = run(
    [
      "--cwd",
      repo,
      "--output",
      join(directory, "audit"),
      "--check",
      "ignored-instructions",
      "--native-directory",
      native,
      "--install",
    ],
    env,
  );
  assert.equal(result.status, 0, result.stderr);
  const manifest = JSON.parse(result.stdout);
  assert.equal(manifest.installed, true);
  assert.ok(manifest.native_file.startsWith(native));
  assert.match(
    await readFile(manifest.native_file, "utf8"),
    /SCRIPTED YAML CHECK AUDIT/,
  );
  await assert.rejects(readdir(ambientProfile));
  assert.ok(manifest.setup_files.some(({ path }) => path === "src/parser.rs"));
  assert.equal(
    await readFile(join(repo, "src/parser.rs")).then(
      () => true,
      () => false,
    ),
    false,
  );
  assert.match(
    await readFile(join(directory, "audit/repo-inputs/src/parser.rs"), "utf8"),
    /empty input/,
  );

  const existingSession = join(native, "protected-session.jsonl");
  await writeFile(existingSession, "keep\n");
  assert.equal(await readFile(existingSession, "utf8"), "keep\n");
});

test("requires an explicit isolated profile for installation", async (t) => {
  const { directory, repo } = await fixture();
  t.after(() => rm(directory, { recursive: true, force: true }));
  await installDefinition(repo, "over-exploring");
  const result = run([
    "--cwd",
    repo,
    "--output",
    join(directory, "audit"),
    "--check",
    "over-exploring",
    "--install",
  ]);
  assert.notEqual(result.status, 0);
  assert.match(
    result.stderr,
    /--install requires an explicit --native-directory/,
  );
});

test("ambient profile variables do not authorize install or cause dry-run writes", async (t) => {
  const { directory, repo } = await fixture();
  t.after(() => rm(directory, { recursive: true, force: true }));
  await installDefinition(repo, "scope-creep");
  const profile = join(directory, "codex-profile");
  const env = { ...process.env, CODEX_HOME: profile };
  delete env.CLAUDE_CONFIG_DIR;
  const result = run(
    [
      "--cwd",
      repo,
      "--output",
      join(directory, "audit"),
      "--check",
      "scope-creep",
      "--vendor",
      "codex",
      "--install",
    ],
    env,
  );
  assert.notEqual(result.status, 0);
  assert.match(
    result.stderr,
    /--install requires an explicit --native-directory/,
  );
  await assert.rejects(readdir(profile));

  const dryRun = run(
    [
      "--cwd",
      repo,
      "--output",
      join(directory, "dry-run"),
      "--check",
      "scope-creep",
      "--vendor",
      "codex",
    ],
    env,
  );
  assert.equal(dryRun.status, 0, dryRun.stderr);
  assert.equal(JSON.parse(dryRun.stdout).native_file, null);
  await assert.rejects(readdir(profile));
});

test("refuses symlinked output and native profile paths", async (t) => {
  const { directory, repo } = await fixture();
  t.after(() => rm(directory, { recursive: true, force: true }));
  await installDefinition(repo, "scope-creep");
  const target = join(directory, "target");
  await mkdir(target);
  const outputLink = join(directory, "output-link");
  await symlink(target, outputLink);
  const outputResult = run([
    "--cwd",
    repo,
    "--output",
    outputLink,
    "--check",
    "scope-creep",
  ]);
  assert.notEqual(outputResult.status, 0);
  assert.match(outputResult.stderr, /symlink/i);

  const outputParentLink = join(directory, "output-parent-link");
  await symlink(target, outputParentLink);
  const nestedOutputResult = run([
    "--cwd",
    repo,
    "--output",
    join(outputParentLink, "new-parent", "audit"),
    "--check",
    "scope-creep",
  ]);
  assert.notEqual(nestedOutputResult.status, 0);
  assert.match(nestedOutputResult.stderr, /symlink/i);

  const profileLink = join(directory, "profile-link");
  await symlink(target, profileLink);
  const nativeResult = run([
    "--cwd",
    repo,
    "--output",
    join(directory, "native-output"),
    "--check",
    "scope-creep",
    "--vendor",
    "codex",
    "--native-directory",
    profileLink,
    "--install",
  ]);
  assert.notEqual(nativeResult.status, 0);
  assert.match(nativeResult.stderr, /symlink/i);

  const nativeParentLink = join(directory, "native-parent-link");
  await symlink(target, nativeParentLink);
  const nestedNativeResult = run([
    "--cwd",
    repo,
    "--output",
    join(directory, "nested-native-output"),
    "--check",
    "scope-creep",
    "--vendor",
    "codex",
    "--native-directory",
    join(nativeParentLink, "new-parent", "sessions"),
    "--install",
  ]);
  assert.notEqual(nestedNativeResult.status, 0);
  assert.match(nestedNativeResult.stderr, /symlink/i);
  assert.deepEqual(await readdir(target), []);
});

test("rejects existing vendor profile destinations outside the fresh audit output", async (t) => {
  const { directory, repo } = await fixture();
  t.after(() => rm(directory, { recursive: true, force: true }));
  await installDefinition(repo, "scope-creep");
  for (const vendor of ["claude", "codex", "pi"]) {
    const profile = join(directory, `${vendor}-profile`, "sessions");
    await mkdir(profile, { recursive: true });
    await writeFile(join(profile, "existing.jsonl"), "keep\n");
    const output = join(directory, `audit-${vendor}`);
    const result = run([
      "--cwd",
      repo,
      "--output",
      output,
      "--check",
      "scope-creep",
      "--vendor",
      vendor,
      "--native-directory",
      profile,
      "--install",
    ]);
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /must be inside the new --output directory/);
    assert.deepEqual(await readdir(profile), ["existing.jsonl"]);
    assert.equal(
      await readFile(join(profile, "existing.jsonl"), "utf8"),
      "keep\n",
    );
    await assert.rejects(readdir(output));
  }
});
