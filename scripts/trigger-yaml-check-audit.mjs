import { randomUUID, randomBytes, createHash } from "node:crypto";
import {
  readFile,
  writeFile,
  mkdir,
  open,
  realpath,
  access,
  rm,
  rmdir,
} from "node:fs/promises";
import { homedir } from "node:os";
import { resolve, join, dirname } from "node:path";
import { fileURLToPath } from "node:url";
import { parseArgs } from "node:util";

const marker = "SCRIPTED YAML CHECK AUDIT";
const { values } = parseArgs({
  options: {
    cwd: { type: "string" },
    output: { type: "string" },
    vendor: { type: "string", default: "claude" },
    install: { type: "boolean", default: false },
  },
});
const cwdOption = values.cwd;
const outputOption = values.output;
const vendor = values.vendor;
if (!["claude", "codex", "pi"].includes(vendor)) {
  throw new Error("Use --vendor claude|codex|pi.");
}
if (!cwdOption || !outputOption) {
  throw new Error(
    "Use --cwd DISPOSABLE_REPOSITORY --output NEW_AUDIT_DIRECTORY [--vendor claude|codex|pi] [--install].",
  );
}
const cwd = await realpath(resolve(cwdOption));
await access(join(cwd, ".git"));
const output = resolve(outputOption);
const now = new Date();
function randomUuidV7(timestamp) {
  const time = timestamp.toString(16).padStart(12, "0").slice(-12);
  const random = randomBytes(10).toString("hex");
  const variant = ((Number.parseInt(random[3], 16) & 0x3) | 0x8).toString(16);
  return `${time.slice(0, 8)}-${time.slice(8)}-7${random.slice(0, 3)}-${variant}${random.slice(4, 7)}-${random.slice(7, 19)}`;
}
const sessionId = vendor === "pi" ? randomUuidV7(now.getTime()) : randomUUID();
const project = cwd.replace(/[^a-zA-Z0-9-]/g, "-");
const piProject = `--${cwd.replace(/^[/\\]+/, "").replace(/[/\\]+/g, "-")}--`;
const isoTimestamp = now.toISOString();
const codexTimestamp = isoTimestamp.slice(0, 19).replaceAll(":", "-");
const piTimestamp = `${codexTimestamp}-${isoTimestamp.slice(20, 23)}Z`;
const nativeDirectories = {
  claude: join(homedir(), ".claude", "projects", project),
  codex: join(
    homedir(),
    ".codex",
    "sessions",
    isoTimestamp.slice(0, 4),
    isoTimestamp.slice(5, 7),
    isoTimestamp.slice(8, 10),
  ),
  pi: join(
    process.env.PI_CODING_AGENT_DIR ??
      process.env.PI_AGENT_DIR ??
      join(homedir(), ".pi", "agent"),
    "sessions",
    piProject,
  ),
};
const nativeDirectory = nativeDirectories[vendor];
const nativeNames = {
  claude: `${sessionId}.jsonl`,
  codex: `rollout-${codexTimestamp}-${sessionId}.jsonl`,
  pi: `${piTimestamp}_${sessionId}.jsonl`,
};
const nativeFile = join(nativeDirectory, nativeNames[vendor]);
const sourceDefinition = resolve(
  dirname(fileURLToPath(import.meta.url)),
  "../.antiburn/checks/failed-retry-loop.yml",
);
const definitionDirectory = join(cwd, ".antiburn", "checks");
const definition = await readFile(sourceDefinition);
const installedDefinition = join(definitionDirectory, "failed-retry-loop.yml");
const existing = await readFile(installedDefinition);
if (!existing.equals(definition)) {
  throw new Error(
    "Install the unchanged Failed retry loop definition in the disposable repository first.",
  );
}
let parent = null;
const start = Date.now() - 8000;
let claudeIndex = 0;
function claudeRecord(type, message) {
  const uuid = randomUUID();
  const result = {
    parentUuid: parent,
    isSidechain: false,
    type,
    message,
    uuid,
    timestamp: new Date(start + claudeIndex++ * 1000).toISOString(),
    cwd,
    sessionId,
    version: "2.1.278",
    entrypoint: "cli",
    userType: "external",
    auditFixture: marker,
  };
  parent = uuid;
  return result;
}
const claudeRows = [
  claudeRecord("user", {
    role: "user",
    content: `${marker}. Make the parser tests pass. Identify and fix the cause of any test failure.`,
  }),
];
for (let attempt = 1; attempt <= 3; attempt++) {
  const call = `audit_${randomUUID()}`;
  claudeRows.push(
    claudeRecord("assistant", {
      id: `msg_${randomUUID()}`,
      type: "message",
      role: "assistant",
      model: "scripted-audit-fixture",
      stop_reason: "tool_use",
      stop_sequence: null,
      usage: { input_tokens: 0, output_tokens: 0 },
      content: [
        {
          type: "tool_use",
          id: call,
          name: "Bash",
          input: {
            command: "cargo test parser",
            description: "Run the parser tests",
          },
        },
      ],
    }),
  );
  claudeRows.push(
    claudeRecord("user", {
      role: "user",
      content: [
        {
          type: "tool_result",
          tool_use_id: call,
          is_error: true,
          content:
            "test parser::tests::parse_empty FAILED: expected an empty list, got null; src/parser.rs:42\nerror: test failed, exit code 101",
        },
      ],
    }),
  );
}
function codexRecord(type, payload) {
  const timestamp = new Date(start + codexIndex++ * 1000).toISOString();
  const row = {
    timestamp,
    type,
    payload,
  };
  return JSON.stringify(row);
}
let codexIndex = 0;
const codexRows = [
  codexRecord("session_meta", {
    id: sessionId,
    timestamp: new Date(start).toISOString(),
    cwd,
    cli_version: "0.160.1",
    source: "cli",
    thread_source: "user",
    history_mode: "legacy",
  }),
  codexRecord("turn_context", {
    model: "scripted-audit-fixture",
    effort: "medium",
  }),
  codexRecord("response_item", {
    type: "message",
    role: "user",
    content: [
      {
        type: "input_text",
        text: `${marker}. Make the parser tests pass. Identify and fix the cause of any test failure.`,
      },
    ],
  }),
];
for (let attempt = 1; attempt <= 3; attempt++) {
  const call = `audit_${randomUUID()}`;
  codexRows.push(
    codexRecord("response_item", {
      type: "function_call",
      name: "exec_command",
      call_id: call,
      arguments: JSON.stringify({ cmd: "cargo test parser" }),
    }),
    codexRecord("response_item", {
      type: "function_call_output",
      id: `audit_result_${attempt}`,
      call_id: call,
      status: "failed",
      output:
        "test parser::tests::parse_empty FAILED: expected an empty list, got null; src/parser.rs:42\nerror: test failed, exit code 101",
    }),
  );
}
function piRecord(type, id, parentId, payload = {}) {
  return JSON.stringify({
    type,
    id,
    parentId,
    timestamp: new Date(start + piIndex++ * 1000).toISOString(),
    ...payload,
  });
}
let piIndex = 1;
const piRows = [
  JSON.stringify({
    type: "session",
    version: 3,
    id: sessionId,
    timestamp: new Date(start).toISOString(),
    cwd,
  }),
];
let piParent = null;
piRows.push(
  piRecord("model_change", "audit-model", piParent, {
    provider: "synthetic",
    modelId: "scripted-audit-fixture",
  }),
);
piParent = "audit-model";
piRows.push(
  piRecord("message", "audit-user", piParent, {
    message: {
      role: "user",
      content: [
        {
          type: "text",
          text: `${marker}. Make the parser tests pass. Identify and fix the cause of any test failure.`,
        },
      ],
    },
  }),
);
piParent = "audit-user";
for (let attempt = 1; attempt <= 3; attempt++) {
  const call = `audit_call_${attempt}`;
  const request = `audit_request_${attempt}`;
  const result = `audit_result_${attempt}`;
  piRows.push(
    piRecord("message", request, piParent, {
      message: {
        role: "assistant",
        model: "scripted-audit-fixture",
        content: [
          {
            type: "toolCall",
            id: call,
            name: "bash",
            arguments: { command: "cargo test parser" },
          },
        ],
      },
    }),
    piRecord("message", result, request, {
      message: {
        role: "toolResult",
        toolCallId: call,
        toolName: "bash",
        isError: true,
        content: [
          {
            type: "text",
            text: "test parser::tests::parse_empty FAILED: expected an empty list, got null; src/parser.rs:42\nerror: test failed, exit code 101",
          },
        ],
      },
    }),
  );
  piParent = result;
}
const rowsByVendor = { claude: claudeRows, codex: codexRows, pi: piRows };
const bytes =
  rowsByVendor[vendor]
    .map((row) => (typeof row === "string" ? row : JSON.stringify(row)))
    .join("\n") + "\n";
const manifest = {
  schema_version: 1,
  kind: "scripted-native-evidence",
  vendor,
  label: marker,
  session_id: sessionId,
  session_id_provenance:
    "Generated by this fixture script; not a native producer identity.",
  cwd,
  native_file: nativeFile,
  installed: values.install,
  definition_sha256: createHash("sha256").update(definition).digest("hex"),
  scripted_results: 3,
  emulated_source_format: {
    claude: "Claude CLI 2.1.278",
    codex: "Codex rollout JSONL 0.160.1",
    pi: "Pi V3 session JSONL, shape based on Pi 0.84.4 characterization",
  }[vendor],
  intended_evaluator: "Jev via the running Antiburn app",
  evaluation_verified: false,
  assessment_seeded: false,
  provider_response_seeded: false,
  limits:
    "The tool calls and results are scripted fixture data. This does not prove spontaneous agent retry behavior or producer-version compatibility.",
};
await mkdir(output);
let nativeCreated = false;
try {
  await writeFile(join(output, "scripted-session.jsonl"), bytes, {
    flag: "wx",
  });
  await writeFile(
    join(output, "manifest.json"),
    JSON.stringify(manifest, null, 2) + "\n",
    { flag: "wx" },
  );
  if (values.install) {
    await mkdir(nativeDirectory, { recursive: true });
    const native = await open(nativeFile, "wx");
    nativeCreated = true;
    try {
      await native.writeFile(bytes);
    } finally {
      await native.close();
    }
  }
  console.log(JSON.stringify(manifest, null, 2));
} catch (error) {
  if (nativeCreated) await rm(nativeFile);
  await rm(join(output, "scripted-session.jsonl"), { force: true });
  await rm(join(output, "manifest.json"), { force: true });
  await rmdir(output);
  throw error;
}
