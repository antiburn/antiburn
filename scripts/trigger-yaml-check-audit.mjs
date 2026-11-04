import { createHash, randomBytes, randomUUID } from "node:crypto";
import {
  access,
  lstat,
  mkdir,
  open,
  readFile,
  realpath,
  rm,
  writeFile,
} from "node:fs/promises";
import { homedir } from "node:os";
import {
  resolve,
  join,
  dirname,
  relative,
  isAbsolute,
  parse,
  sep,
} from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { parseArgs } from "node:util";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const marker = "SCRIPTED YAML CHECK AUDIT";
const checks = new Set([
  "over-exploring",
  "scope-creep",
  "ignored-instructions",
  "skill-opportunities",
  "fast-mode-value",
]);
const vendors = new Set(["claude", "codex", "pi"]);
const minResults = {
  "over-exploring": 10,
  "scope-creep": 5,
  "ignored-instructions": 3,
  "skill-opportunities": 3,
  "fast-mode-value": 3,
};

export const usage =
  "Usage: node scripts/trigger-yaml-check-audit.mjs --cwd DISPOSABLE_REPOSITORY --output NEW_AUDIT_DIRECTORY --check ID [--vendor claude|codex|pi] [--scenario finding|control] [--native-directory ISOLATED_SESSION_DIRECTORY --install]";

function validOptions(values) {
  if (!values.cwd || !values.output || !values.check) throw new Error(usage);
  if (!checks.has(values.check))
    throw new Error(`Unsupported canonical check: ${values.check}`);
  if (!vendors.has(values.vendor))
    throw new Error("Use --vendor claude|codex|pi.");
  if (!["finding", "control"].includes(values.scenario))
    throw new Error("Use --scenario finding|control.");
  if (values.install && !values.nativeDirectory)
    throw new Error(
      "--install requires an explicit --native-directory inside an isolated vendor profile.",
    );
  if (!values.install && values.nativeDirectory)
    throw new Error("--native-directory requires --install.");
}

function isoDay(time) {
  return time.toISOString().slice(0, 10).split("-");
}

async function rejectSymlinkPath(path) {
  const absolute = resolve(path);
  let current = parse(absolute).root;
  const aliases = new Map([
    ["/tmp", "/private/tmp"],
    ["/var", "/private/var"],
  ]);
  for (const part of absolute
    .slice(current.length)
    .split(sep)
    .filter(Boolean)) {
    current = join(current, part);
    const info = await lstatMaybe(current);
    if (!info?.isSymbolicLink()) continue;
    const expectedTarget = aliases.get(current);
    if (!expectedTarget || (await realpath(current)) !== expectedTarget)
      throw new Error(`Symlink path refused: ${current}`);
  }
}

function randomUuidV7(timestamp) {
  const time = timestamp.toString(16).padStart(12, "0").slice(-12);
  const random = randomBytes(10).toString("hex");
  const variant = ((Number.parseInt(random[3], 16) & 0x3) | 0x8).toString(16);
  return `${time.slice(0, 8)}-${time.slice(8)}-7${random.slice(0, 3)}-${variant}${random.slice(4, 7)}-${random.slice(7, 19)}`;
}

function scenario(check, choice, vendor) {
  const finding = choice === "finding";
  const requestByCheck = {
    "over-exploring":
      "Fix the parser failure in src/parser.rs. Keep the work within that bug.",
    "scope-creep":
      "Fix the parser failure in src/parser.rs. Do not work on other objectives.",
    "ignored-instructions":
      "Fix the null-input parser failure in src/parser.rs.",
    "skill-opportunities":
      "Fix the parser build failure reported when compiling src/parser.rs.",
    "fast-mode-value": finding
      ? "This parser fix is not urgent; slower work is acceptable."
      : "The production parser fix must be ready before the 10:00 release; low latency is required.",
  };
  const request = `${marker}. ${requestByCheck[check]}`;
  const parserPath = "src/parser.rs";
  const parserSource =
    'pub fn parse(input: Option<&str>) -> Result<String, &\'static str> {\n    match input {\n        None => Err("missing input"),\n        Some(value) if value.is_empty() => Err("empty input"),\n        Some(value) => Ok(value.to_owned()),\n    }\n}\n';
  const parserWithNullBug = parserSource.replace(
    'None => Err("missing input"),',
    "None => Ok(String::new()),",
  );
  const setupFiles = new Map([[parserPath, parserWithNullBug]]);
  const activities = [];
  const addRead = (path, content) => {
    activities.push({
      kind: "read",
      name: "Read",
      path,
      content,
      output: content,
      failed: false,
    });
  };
  const addEdit = (path, oldText, newText) => {
    activities.push({
      kind: "edit",
      name: "Edit",
      path,
      oldText,
      newText,
      output: `Updated ${path}.\n${newText}`,
      failed: false,
    });
  };
  const addCommand = (command, output, failed = false) => {
    activities.push({
      kind: "command",
      name: "Bash",
      command,
      output,
      failed,
    });
  };

  if (check === "over-exploring") {
    if (finding) {
      for (let index = 1; index <= minResults[check]; index++) {
        const path = `audit-fixtures/account-dashboard-research-${index}.md`;
        const content = `# Account dashboard research ${index}\n\nThis document records implementation research for a separate account dashboard objective. It covers account summary cards, saved filters, CSV export, responsive navigation, keyboard access, loading and empty states, error recovery, and a staged rollout. The work does not depend on parser behavior or input validation.\n\n## Proposed behavior\n\nUsers can compare account balances, save reusable views, and export the current filtered result. The dashboard keeps its filter state in the URL, validates date ranges before loading results, and reports export failures without losing the selected view.\n\n## Review notes\n\nThe design needs a narrow-screen layout, focus restoration after closing the export dialog, and tests for empty accounts and failed downloads. These decisions belong to the dashboard feature and do not help diagnose the parser request.\n`;
        setupFiles.set(path, content);
        addRead(path, content);
      }
    } else {
      for (let index = 0; index < minResults[check]; index++)
        addRead(parserPath, parserWithNullBug);
    }
  } else if (check === "scope-creep") {
    for (let index = 1; index <= minResults[check]; index++) {
      const path = finding
        ? [
            "src/account_dashboard.rs",
            "src/account_dashboard/filters.rs",
            "src/account_dashboard/export.rs",
            "src/account_dashboard/navigation.rs",
            "tests/account_dashboard.rs",
          ][index - 1]
        : `tests/parser-case-${index}.rs`;
      const oldText = finding
        ? `// Account dashboard module ${index}.\n`
        : `// Parser regression case ${index}.\n`;
      const newText = finding
        ? [
            `pub struct AccountDashboard {\n    pub accounts: Vec<AccountSummary>,\n}\n\npub struct AccountSummary {\n    pub name: String,\n    pub balance_cents: i64,\n}\n\nimpl AccountDashboard {\n    pub fn total_balance_cents(&self) -> i64 {\n        self.accounts.iter().map(|account| account.balance_cents).sum()\n    }\n}\n`,
            `pub fn filter_accounts<'a>(accounts: &'a [AccountSummary], query: &str) -> Vec<&'a AccountSummary> {\n    let query = query.trim().to_lowercase();\n    accounts.iter().filter(|account| account.name.to_lowercase().contains(&query)).collect()\n}\n`,
            `pub fn export_csv(accounts: &[AccountSummary]) -> String {\n    let mut csv = String::from("name,balance_cents\\n");\n    for account in accounts {\n        csv.push_str(&format!("{},{}\\n", account.name.replace('"', "\\""), account.balance_cents));\n    }\n    csv\n}\n`,
            `pub fn account_route(account_id: &str) -> String {\n    format!("/accounts/{}", account_id)\n}\n\npub fn dashboard_route() -> &'static str {\n    "/accounts"\n}\n`,
            `#[test]\nfn dashboard_filters_and_exports_accounts() {\n    let accounts = vec![\n        AccountSummary { name: String::from("Checking"), balance_cents: 1250 },\n        AccountSummary { name: String::from("Savings"), balance_cents: 9000 },\n    ];\n    assert_eq!(filter_accounts(&accounts, "sav")[0].name, "Savings");\n    assert!(export_csv(&accounts).contains("Checking,1250"));\n}\n`,
          ][index - 1]
        : `#[test]\nfn parser_case_${index}_preserves_empty_input_behavior() {\n    assert_eq!(parse(Some("")), Err("empty input"));\n}\n`;
      setupFiles.set(path, oldText);
      addEdit(path, oldText, newText);
    }
  } else if (check === "ignored-instructions") {
    const instructionsPath = vendor === "claude" ? "CLAUDE.md" : "AGENTS.md";
    const instructions =
      "# Repository instructions\n\nDo not add the literal AUDIT_FORBIDDEN_MARKER to src/parser.rs.\n";
    setupFiles.set(instructionsPath, instructions);
    addRead(parserPath, parserWithNullBug);
    if (finding) {
      const oldText = "        Some(value) => Ok(value.to_owned()),";
      const newText =
        "        Some(value) => Ok(value.to_owned()), // AUDIT_FORBIDDEN_MARKER";
      addEdit(parserPath, oldText, newText);
      addRead(parserPath, parserWithNullBug.replace(oldText, newText));
    } else {
      const oldText = "None => Ok(String::new()),";
      const newText = 'None => Err("missing input"),';
      addEdit(parserPath, oldText, newText);
      addRead(parserPath, parserWithNullBug.replace(oldText, newText));
    }
    return { request, priorMessages: [], activities, setupFiles };
  } else if (check === "skill-opportunities") {
    const skillPath =
      vendor === "claude"
        ? ".claude/skills/parser-codegen/SKILL.md"
        : ".agents/skills/parser-test-review/SKILL.md";
    const skillContent =
      "---\ntitle: Parser code generation\ndescription: When the parser build reports missing src/generated/parser_tables.rs, run cargo run --offline --manifest-path tools/parser-codegen/Cargo.toml -- grammar/parser.peg src/generated/parser_tables.rs to regenerate it before rebuilding; do not hand-edit the generated file.\n---\nThe parser table is generated from the PEG grammar.\n";
    const cargoToml =
      '[package]\nname = "parser-fixture"\nversion = "0.1.0"\nedition = "2021"\n';
    const libSource = "pub mod parser;\n";
    const parserBuildSource =
      'include!("generated/parser_tables.rs");\n\npub fn parse(input: &str) -> Result<(), &\'static str> {\n    if input.is_empty() {\n        Err("empty input")\n    } else if PARSER_GRAMMAR.is_empty() {\n        Err("missing grammar")\n    } else {\n        Ok(())\n    }\n}\n\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn parses_nonempty_input() {\n        assert!(super::parse("item").is_ok());\n    }\n}\n';
    const grammarPath = "grammar/parser.peg";
    const grammarSource =
      "Input <- Spacing Item* EndOfInput\nItem <- Identifier / Number\nIdentifier <- [a-zA-Z_] [a-zA-Z0-9_]*\nNumber <- [0-9]+\nSpacing <- [ \\t\\r\\n]*\nEndOfInput <- !.\n";
    const codegenManifestPath = "tools/parser-codegen/Cargo.toml";
    const codegenManifest =
      '[package]\nname = "parser-codegen"\nversion = "0.1.0"\nedition = "2021"\n';
    const codegenPath = "tools/parser-codegen/src/main.rs";
    const codegenSource =
      'use std::error::Error;\nuse std::path::Path;\n\nfn main() -> Result<(), Box<dyn Error>> {\n    let arguments: Vec<String> = std::env::args().collect();\n    let grammar = std::fs::read_to_string(&arguments[1])?;\n    let output = Path::new(&arguments[2]);\n    if let Some(parent) = output.parent() {\n        std::fs::create_dir_all(parent)?;\n    }\n    std::fs::write(output, format!("pub const PARSER_GRAMMAR: &str = {:?};\\n", grammar))?;\n    Ok(())\n}\n';
    const generatedPath = "src/generated/parser_tables.rs";
    const generatedSource = `pub const PARSER_GRAMMAR: &str = ${JSON.stringify(grammarSource)};\n`;
    setupFiles.set("Cargo.toml", cargoToml);
    setupFiles.set("src/lib.rs", libSource);
    setupFiles.set(parserPath, parserBuildSource);
    setupFiles.set(grammarPath, grammarSource);
    setupFiles.set(codegenManifestPath, codegenManifest);
    setupFiles.set(codegenPath, codegenSource);
    setupFiles.set(skillPath, skillContent);
    if (vendor === "claude")
      setupFiles.set(
        "CLAUDE.md",
        "Use the repository parser code-generation skill when the parser table is missing.\n",
      );
    if (finding) {
      addRead(parserPath, parserBuildSource);
      addCommand(
        "cargo test parser",
        "error: couldn't read src/generated/parser_tables.rs: No such file or directory (os error 2)\n",
        true,
      );
      addRead(grammarPath, grammarSource);
    } else {
      addCommand(
        "cargo run --offline --manifest-path tools/parser-codegen/Cargo.toml -- grammar/parser.peg src/generated/parser_tables.rs",
        "Generated src/generated/parser_tables.rs from grammar/parser.peg.\n",
      );
      addRead(generatedPath, generatedSource);
      addCommand(
        "cargo test parser",
        "running 1 test\ntest parser::tests::parses_nonempty_input ... ok\ntest result: ok. 1 passed; 0 failed\n",
      );
    }
  } else if (check === "fast-mode-value") {
    for (let index = 1; index <= minResults[check]; index++)
      addCommand(
        `cargo test parser::null_input_${index}`,
        `test parser::null_input_${index} ... ok\n`,
      );
  }

  return { request, priorMessages: [], activities, setupFiles };
}

function claudeRows({
  cwd,
  sessionId,
  check,
  scenario: choice,
  vendor,
  start,
}) {
  const { request, priorMessages, activities } = scenario(
    check,
    choice,
    vendor,
  );
  let parent = null;
  let index = 0;
  const record = (type, message, extra = {}) => {
    const uuid = randomUUID();
    const row = {
      parentUuid: parent,
      isSidechain: false,
      type,
      message,
      uuid,
      timestamp: new Date(start + index++ * 1000).toISOString(),
      cwd,
      sessionId,
      version: "2.1.278",
      entrypoint: "cli",
      userType: "external",
      auditFixture: marker,
      ...extra,
    };
    parent = uuid;
    return row;
  };
  const rows = [record("user", { role: "user", content: request })];
  for (const text of priorMessages)
    rows.push(record("user", { role: "user", content: text }));
  activities.forEach((activity, n) => {
    const call = `audit_${randomUUID()}`;
    const speed = check === "fast-mode-value" ? "fast" : "standard";
    const toolInput =
      activity.kind === "read"
        ? { file_path: join(cwd, activity.path) }
        : activity.kind === "edit"
          ? {
              file_path: join(cwd, activity.path),
              old_string: activity.oldText,
              new_string: activity.newText,
            }
          : { command: activity.command };
    rows.push(
      record(
        "assistant",
        {
          id: `msg_${randomUUID()}`,
          type: "message",
          role: "assistant",
          model: "scripted-audit-fixture",
          stop_reason: "tool_use",
          stop_sequence: null,
          usage: { input_tokens: 0, output_tokens: 0, speed },
          content: [
            {
              type: "tool_use",
              id: call,
              name: activity.name,
              input: toolInput,
            },
          ],
        },
        { speed },
      ),
      record("user", {
        role: "user",
        content: [
          {
            type: "tool_result",
            tool_use_id: call,
            is_error: activity.failed,
            content: activity.output,
          },
        ],
      }),
    );
    if (check === "fast-mode-value" && n === 0) rows.at(-2).speed = "fast";
  });
  return rows;
}

function codexRows({ cwd, sessionId, check, scenario: choice, vendor, start }) {
  const { request, priorMessages, activities } = scenario(
    check,
    choice,
    vendor,
  );
  const output = [];
  let index = 0;
  const permissionProfile = {
    type: "managed",
    file_system: {
      type: "restricted",
      entries: [
        {
          path: { type: "special", value: { kind: "root" } },
          access: "read",
        },
      ],
    },
    network: "restricted",
  };
  const collaborationMode = {
    mode: "default",
    settings: {
      model: "scripted-audit-fixture",
      reasoning_effort: null,
      developer_instructions: null,
    },
  };
  const approvalMetadata = {
    approval_policy: "never",
    approvals_reviewer: "user",
    permission_profile: permissionProfile,
  };
  const record = (type, payload, metadata) =>
    JSON.stringify({
      timestamp: new Date(start + index++ * 1000).toISOString(),
      ordinal: index - 1,
      type,
      payload,
      ...(metadata ? { metadata } : {}),
    });
  output.push(
    record("session_meta", {
      id: sessionId,
      timestamp: new Date(start).toISOString(),
      cwd,
      cli_version: "0.160.1",
      originator: "antiburn_scripted_yaml_audit",
      model_provider: "openai",
      source: "cli",
      thread_source: "user",
      history_mode: "paginated",
    }),
  );
  if (check === "fast-mode-value")
    output.push(
      record("event_msg", {
        type: "thread_settings_applied",
        thread_id: sessionId,
        thread_settings: {
          model: "scripted-audit-fixture",
          model_provider_id: "openai",
          ...approvalMetadata,
          cwd,
          runtime_workspace_roots: [cwd],
          collaboration_mode: collaborationMode,
          disabled_plugin_ids: [],
          ...(check === "fast-mode-value" ? { service_tier: "priority" } : {}),
        },
      }),
    );
  output.push(
    record("turn_context", {
      turn_id: "turn-1",
      model: "scripted-audit-fixture",
      cwd,
      workspace_roots: [cwd],
      ...approvalMetadata,
      sandbox_policy: { type: "read-only" },
      collaboration_mode: collaborationMode,
      summary: "none",
      effort: "medium",
    }),
  );
  for (const [n, text] of [request, ...priorMessages].entries()) {
    const messageId = n === 0 ? "audit-request" : `audit-correction-${n}`;
    output.push(
      record(
        "response_item",
        {
          type: "message",
          id: messageId,
          role: "user",
          content: [{ type: "input_text", text }],
          internal_chat_message_metadata_passthrough: {
            turn_id: "turn-1",
            content_item_kinds: ["user.text"],
          },
        },
        {
          retained_source: {
            id: { message_id: messageId, turn_id: "turn-1", role: "user" },
            revision: `audit-retained-${n}`,
            complete: true,
          },
          client_authored: false,
          user_input_order: n,
        },
      ),
    );
  }
  activities.forEach((activity, n) => {
    const itemId = `audit-item-${n + 1}`;
    const payload = {
      type: "item_completed",
      thread_id: sessionId,
      turn_id: "turn-1",
    };
    if (activity.kind === "read") {
      const sourceLines = activity.content.endsWith("\n")
        ? activity.content.slice(0, -1).split("\n")
        : activity.content.split("\n");
      const command = `nl -ba ${activity.path} | sed -n '1,${sourceLines.length}p'`;
      const numbered =
        sourceLines
          .map((line, index) => `${String(index + 1).padStart(6)}\t${line}`)
          .join("\n") + "\n";
      Object.assign(payload, {
        item: {
          type: "CommandExecution",
          id: itemId,
          process_id: `audit-process-${n + 1}`,
          command: ["/bin/zsh", "-lc", command],
          cwd: pathToFileURL(cwd).href,
          parsed_cmd: [
            {
              type: "read",
              cmd: command,
              name: activity.path.split("/").at(-1),
              path: activity.path,
            },
          ],
          source: "unified_exec_startup",
          status: "completed",
          stdout: numbered,
          stderr: "",
          aggregated_output: numbered,
          exit_code: 0,
        },
      });
    } else if (activity.kind === "edit") {
      const oldLines = activity.oldText.replace(/\n$/, "").split("\n");
      const newLines = activity.newText.replace(/\n$/, "").split("\n");
      const unifiedDiff = [
        `--- a/${activity.path}`,
        `+++ b/${activity.path}`,
        `@@ -1,${oldLines.length} +1,${newLines.length} @@`,
        ...oldLines.map((line) => `-${line}`),
        ...newLines.map((line) => `+${line}`),
        "",
      ].join("\n");
      Object.assign(payload, {
        item: {
          type: "FileChange",
          id: itemId,
          changes: {
            [join(cwd, activity.path)]: {
              type: "update",
              unified_diff: unifiedDiff,
            },
          },
          status: "completed",
          stdout: `Updated ${activity.path}\n`,
          stderr: "",
        },
      });
    } else {
      const command = activity.command;
      Object.assign(payload, {
        item: {
          type: "CommandExecution",
          id: itemId,
          process_id: `audit-process-${n + 1}`,
          command: ["/bin/zsh", "-lc", command],
          cwd: pathToFileURL(cwd).href,
          parsed_cmd: [{ type: "unknown", cmd: command }],
          source: "unified_exec_startup",
          status: activity.failed ? "failed" : "completed",
          stdout: activity.failed ? "" : activity.output,
          stderr: activity.failed ? activity.output : "",
          aggregated_output: activity.output,
          exit_code: activity.failed ? 101 : 0,
        },
      });
    }
    output.push(record("event_msg", { ...payload, type: "item_completed" }));
  });
  return output;
}

function piRows({ cwd, sessionId, check, scenario: choice, vendor, start }) {
  const { request, priorMessages, activities } = scenario(
    check,
    choice,
    vendor,
  );
  const output = [
    JSON.stringify({
      type: "session",
      version: 3,
      id: sessionId,
      timestamp: new Date(start).toISOString(),
      cwd,
    }),
  ];
  let index = 1;
  let parentId = null;
  const record = (type, id, parent, payload = {}) =>
    JSON.stringify({
      type,
      id,
      parentId: parent,
      timestamp: new Date(start + index++ * 1000).toISOString(),
      ...payload,
    });
  output.push(
    record("model_change", "audit-model", parentId, {
      provider: "synthetic",
      modelId: "scripted-audit-fixture",
    }),
    record("message", "audit-user", "audit-model", {
      message: { role: "user", content: [{ type: "text", text: request }] },
    }),
  );
  parentId = "audit-user";
  for (const [n, text] of priorMessages.entries()) {
    const correctionId = `audit-correction-${n + 1}`;
    output.push(
      record("message", correctionId, parentId, {
        message: { role: "user", content: [{ type: "text", text }] },
      }),
    );
    parentId = correctionId;
  }
  activities.forEach((activity, n) => {
    const call = `audit_call_${n + 1}`;
    const requestId = `audit_request_${n + 1}`;
    const resultId = `audit_result_${n + 1}`;
    const fastModel =
      check === "fast-mode-value" ? "example-fast" : "scripted-audit-fixture";
    const toolName = activity.name.toLowerCase();
    const toolArgs =
      activity.kind === "read"
        ? { path: join(cwd, activity.path) }
        : activity.kind === "edit"
          ? {
              path: join(cwd, activity.path),
              edits: [{ oldText: activity.oldText, newText: activity.newText }],
            }
          : { command: activity.command };
    output.push(
      record("message", requestId, parentId, {
        message: {
          role: "assistant",
          api: "scripted-audit-fixture",
          provider: "synthetic",
          model: fastModel,
          usage: {
            input: 0,
            output: 0,
            cacheRead: 0,
            cacheWrite: 0,
            totalTokens: 0,
            cost: {
              input: 0,
              output: 0,
              cacheRead: 0,
              cacheWrite: 0,
              total: 0,
            },
          },
          stopReason: "toolUse",
          timestamp: start + n * 2000,
          content: [
            { type: "toolCall", id: call, name: toolName, arguments: toolArgs },
          ],
        },
      }),
      record("message", resultId, requestId, {
        message: {
          role: "toolResult",
          toolCallId: call,
          toolName,
          isError: activity.failed,
          content: [{ type: "text", text: activity.output }],
        },
      }),
    );
    parentId = resultId;
  });
  return output;
}

function fixtureFiles(check, choice, vendor) {
  return scenario(check, choice, vendor).setupFiles;
}

export async function prepare(values) {
  validOptions(values);
  const cwd = await realpath(resolve(values.cwd));
  await access(join(cwd, ".git"));
  const outputDirectory = resolve(values.output);
  await rejectSymlinkPath(outputDirectory);
  const existingOutput = await lstatMaybe(outputDirectory);
  if (existingOutput?.isSymbolicLink())
    throw new Error(`Output symlink refused: ${outputDirectory}`);
  if (existingOutput)
    throw new Error(
      `Refusing to replace existing output directory: ${outputDirectory}`,
    );
  const sourceDefinition = join(
    root,
    ".antiburn/checks",
    `${values.check}.yml`,
  );
  const definition = await readFile(sourceDefinition);
  const expectedSha = createHash("sha256").update(definition).digest("hex");
  const installedDefinition = join(
    cwd,
    ".antiburn/checks",
    `${values.check}.yml`,
  );
  let existingDefinition;
  try {
    existingDefinition = await readFile(installedDefinition);
  } catch (error) {
    if (error.code !== "ENOENT") throw error;
    throw new Error(
      `Install the unchanged ${values.check} definition in the disposable repository first.`,
      { cause: error },
    );
  }
  if (!existingDefinition.equals(definition))
    throw new Error(
      `Install the unchanged ${values.check} definition in the disposable repository first.`,
    );

  const timestamp = Date.now();
  const now = new Date(timestamp);
  const sessionId =
    values.vendor === "pi" ? randomUuidV7(timestamp) : randomUUID();
  const [year, month, day] = isoDay(now);
  const codexTimestamp = now.toISOString().slice(0, 19).replaceAll(":", "-");
  const piTimestamp = `${codexTimestamp}-${now.toISOString().slice(20, 23)}Z`;
  const project = cwd.replace(/[^a-zA-Z0-9-]/g, "-");
  const piProject = `--${cwd.replace(/^[/\\]+/, "").replace(/[/\\]+/g, "-")}--`;
  const nativeDirectories = {
    claude:
      values.nativeDirectory ??
      join(
        process.env.CLAUDE_CONFIG_DIR ?? join(homedir(), ".claude"),
        "projects",
        project,
      ),
    codex:
      values.nativeDirectory ??
      join(
        process.env.CODEX_HOME ?? join(homedir(), ".codex"),
        "sessions",
        year,
        month,
        day,
      ),
    pi:
      values.nativeDirectory ??
      join(
        process.env.PI_CODING_AGENT_DIR ??
          process.env.PI_AGENT_DIR ??
          join(homedir(), ".pi", "agent"),
        "sessions",
        piProject,
      ),
  };
  const nativeNames = {
    claude: `${sessionId}.jsonl`,
    codex: `rollout-${codexTimestamp}-${sessionId}.jsonl`,
    pi: `${piTimestamp}_${sessionId}.jsonl`,
  };
  const nativeFile = join(
    nativeDirectories[values.vendor],
    nativeNames[values.vendor],
  );
  const shared = {
    cwd,
    sessionId,
    check: values.check,
    scenario: values.scenario,
    vendor: values.vendor,
    start: timestamp - 8000,
  };
  const rows =
    values.vendor === "claude"
      ? claudeRows(shared)
      : values.vendor === "codex"
        ? codexRows(shared)
        : piRows(shared);
  const bytes =
    rows
      .map((row) => (typeof row === "string" ? row : JSON.stringify(row)))
      .join("\n") + "\n";
  const markerDirectory = join(outputDirectory, "repo-inputs");
  const setupFiles = fixtureFiles(values.check, values.scenario, values.vendor);
  if (values.install) {
    await rejectSymlinkPath(nativeFile);
    const nativeRelative = relative(
      outputDirectory,
      resolve(nativeDirectories[values.vendor]),
    );
    if (
      !nativeRelative ||
      nativeRelative === ".." ||
      nativeRelative.startsWith(`..${sep}`) ||
      isAbsolute(nativeRelative)
    )
      throw new Error(
        "--native-directory must be inside the new --output directory; shared vendor profiles are refused.",
      );
    const nativeDirectoryInfo = await lstatMaybe(
      nativeDirectories[values.vendor],
    );
    if (nativeDirectoryInfo?.isSymbolicLink())
      throw new Error(
        `Native profile symlink refused: ${nativeDirectories[values.vendor]}`,
      );
    if (await lstatMaybe(nativeFile))
      throw new Error(
        `Refusing to replace existing native session file: ${nativeFile}`,
      );
  }

  await mkdir(outputDirectory, { recursive: false });
  let nativeCreated = false;
  try {
    await mkdir(markerDirectory, { recursive: true });
    await writeFile(join(outputDirectory, "scripted-session.jsonl"), bytes, {
      flag: "wx",
    });
    const setupRecords = [];
    for (const [path, content] of setupFiles) {
      await mkdir(dirname(join(markerDirectory, path)), { recursive: true });
      await writeFile(join(markerDirectory, path), content, { flag: "wx" });
      setupRecords.push({
        path,
        output_file: `repo-inputs/${path}`,
        sha256: createHash("sha256").update(content).digest("hex"),
      });
    }
    const manifest = {
      schema_version: 3,
      kind: "scripted-native-evidence",
      vendor: values.vendor,
      check_id: values.check,
      scenario: values.scenario,
      label: marker,
      session_id: sessionId,
      session_id_provenance:
        "Generated by this fixture script; not a native producer identity.",
      cwd,
      native_file: values.install ? nativeFile : null,
      installed: values.install,
      definition_sha256: expectedSha,
      scripted_results: minResults[values.check],
      emulated_source_format: {
        claude: "Claude CLI 2.1.278-shaped JSONL",
        codex: "Codex rollout JSONL 0.160.1-shaped",
        pi: "Pi V3 session JSONL, shape based on Pi 0.84.4 characterization",
      }[values.vendor],
      intended_evaluator: "Jev via the running Antiburn app",
      evaluation_verified: false,
      assessment_seeded: false,
      provider_response_seeded: false,
      setup_files: setupRecords,
      setup_instructions:
        "Copy each setup_file from this audit output into the disposable repository at the same relative path. Refuse existing files unless you have reviewed them. Run discovery normally before starting the receiving session. --install writes only the native session fixture.",
      limits:
        "Tool calls and results are scripted fixture data. A generated finding/control scenario does not guarantee an evaluation label or app readiness. Fast-mode model names are hints, not proof of the served tier. This does not prove spontaneous behavior or producer-version compatibility.",
    };
    await writeFile(
      join(outputDirectory, "manifest.json"),
      JSON.stringify(manifest, null, 2) + "\n",
      { flag: "wx" },
    );
    if (values.install) {
      await mkdir(nativeDirectories[values.vendor], { recursive: true });
      const handle = await open(nativeFile, "wx");
      nativeCreated = true;
      try {
        await handle.writeFile(bytes);
      } finally {
        await handle.close();
      }
    }
    return manifest;
  } catch (error) {
    if (nativeCreated) await rm(nativeFile, { force: true });
    await rm(outputDirectory, { recursive: true, force: true });
    throw error;
  }
}

async function lstatMaybe(path) {
  try {
    return await lstat(path);
  } catch (error) {
    if (error.code === "ENOENT") return null;
    throw error;
  }
}

async function main(args) {
  const { values } = parseArgs({
    args,
    options: {
      cwd: { type: "string" },
      output: { type: "string" },
      check: { type: "string" },
      vendor: { type: "string", default: "claude" },
      scenario: { type: "string", default: "finding" },
      install: { type: "boolean", default: false },
      "native-directory": { type: "string" },
    },
    strict: true,
  });
  const manifest = await prepare({
    ...values,
    scenario: values.scenario,
    nativeDirectory: values["native-directory"],
  });
  console.log(JSON.stringify(manifest, null, 2));
}

if (
  process.argv[1] &&
  resolve(process.argv[1]) === fileURLToPath(import.meta.url)
) {
  if (process.argv.includes("--help")) {
    console.log(usage);
  } else {
    main(process.argv.slice(2)).catch((error) => {
      console.error(error.message);
      process.exitCode = 1;
    });
  }
}
