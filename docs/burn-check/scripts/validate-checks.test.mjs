import assert from "node:assert/strict";
import { readFile, readdir } from "node:fs/promises";
import { dirname, extname, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";
import {
  createValidator,
  parseCheck,
  validationBoundary,
} from "./validate-checks.mjs";

const validate = await createValidator();
const choice = {
  type: "choice",
  question: "Is the result useful?",
  criteria: { yes: "Useful.", no: "Not useful." },
};
const base = () => ({
  name: "Example",
  on: { tool_result: null, window: { size: 5, min: 3 } },
  decisions: {
    review: {
      input: { activity: "${{ window.messages }}" },
      output: { verdict: choice },
    },
  },
  actions: [{ type: "status", input: { result: "failure" } }],
});
const validateObject = (value) => validate(JSON.stringify(value));
const docsRoot = fileURLToPath(new URL("../", import.meta.url));
const expressionCorpus = JSON.parse(
  await readFile(
    new URL("../tests/expression-parity.json", import.meta.url),
    "utf8",
  ),
);

test("shared language corpus retains the reviewed admission baseline", () => {
  assert.equal(expressionCorpus.cases.length, 76);
  assert.equal(
    expressionCorpus.cases.filter((entry) => entry.yaml_valid !== false).length,
    75,
  );
  assert.equal(
    expressionCorpus.cases.filter((entry) => entry.schema_valid).length,
    52,
  );
  assert.equal(
    expressionCorpus.cases.filter((entry) => entry.compile_valid).length,
    29,
  );
});

test("shared language corpus separates YAML, schema, and compiler admission", () => {
  const ids = new Set();
  for (const entry of expressionCorpus.cases) {
    assert.equal(typeof entry.id, "string");
    assert.equal(ids.has(entry.id), false, entry.id);
    ids.add(entry.id);
    assert.equal(typeof entry.feature, "string", entry.id);
    assert.equal(typeof entry.source, "string", entry.id);
    assert.equal(typeof entry.schema_valid, "boolean", entry.id);
    assert.equal(typeof entry.compile_valid, "boolean", entry.id);
    const yamlValid = entry.yaml_valid ?? true;
    assert.equal(typeof yamlValid, "boolean", entry.id);
    if (yamlValid) {
      assert.doesNotThrow(() => parseCheck(entry.source), entry.id);
    } else {
      assert.equal(entry.schema_valid, false, entry.id);
      assert.equal(entry.compile_valid, false, entry.id);
      assert.throws(() => parseCheck(entry.source), entry.id);
    }
  }
});

for (const entry of expressionCorpus.cases) {
  test(`language corpus YAML/schema: ${entry.id}`, () => {
    if (entry.schema_valid) {
      assert.doesNotThrow(() => validate(entry.source));
    } else {
      assert.throws(() => validate(entry.source));
    }
  });
}

async function markdownFiles(directory = docsRoot) {
  const files = [];
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    const path = resolve(directory, entry.name);
    if (entry.isDirectory()) {
      if (entry.name !== "node_modules") {
        files.push(...(await markdownFiles(path)));
      }
    } else if (extname(entry.name) === ".md") {
      files.push(path);
    }
  }
  return files;
}

function markdownHeadings(source) {
  const counts = new Map();
  return new Set(
    source.split("\n").flatMap((line) => {
      const heading = line.match(/^#{1,6}\s+(.+?)\s*#*$/)?.[1];
      if (!heading) {
        return [];
      }
      const base = heading
        .replaceAll("`", "")
        .toLowerCase()
        .replace(/[^\p{L}\p{N}\s-]/gu, "")
        .trim()
        .replace(/\s+/g, "-");
      const count = counts.get(base) ?? 0;
      counts.set(base, count + 1);
      return [count === 0 ? base : `${base}-${count}`];
    }),
  );
}

test("validates the canonical Draft 2020-12 schema", async () => {
  const schema = JSON.parse(
    await readFile(
      new URL("../schemas/burn-check.schema.json", import.meta.url),
      "utf8",
    ),
  );
  assert.equal(schema.$schema, "https://json-schema.org/draft/2020-12/schema");
  assert.equal(schema.title, "Burn Checks v1-alpha");
  assert.equal("$id" in schema, false);
  assert.doesNotThrow(() => validate(JSON.stringify(base())));
});

test("draft categories remain schema-valid without promising runtime support", () => {
  for (const category of [
    "shell",
    "read",
    "search",
    "edit",
    "other",
    "write",
    "delegate",
    "skill",
    "mcp",
  ]) {
    const definition = base();
    definition.on.tool_result = { tools: [category] };
    assert.doesNotThrow(() => validateObject(definition), category);
  }
  const definition = base();
  definition.on.tool_result = { tools: ["unknown"] };
  assert.throws(() => validateObject(definition));
});

test("YAML 1.2 keeps on, yes, and no as strings unless explicitly tagged", () => {
  assert.deepEqual(parseCheck("on: yes\noff: no\nflag: true\n"), {
    on: "yes",
    off: "no",
    flag: true,
  });
  assert.deepEqual(parseCheck("value: !!str 12\nflag: !!bool true\n"), {
    value: "12",
    flag: true,
  });
});

test("allows integer-form explicit floats without changing ordinary integers", () => {
  assert.deepEqual(
    parseCheck("zero: !!float 0\nnegative: !!float -1\ninteger: 2\n"),
    {
      zero: 0,
      negative: -1,
      integer: 2,
    },
  );
});

for (const [label, source] of Object.entries({
  "duplicate root keys": "name: first\nname: second\n",
  "duplicate nested keys": "outer: {key: first, key: second}\n",
  "numeric key": "1: value\n",
  "Boolean key": "true: value\n",
  "complex key": "? [a, b]\n: value\n",
  "positive infinity": "value: .inf\n",
  "negative infinity": "value: -.Inf\n",
  NaN: "value: .nan\n",
  "numeric overflow": "value: 1e999\n",
  "custom scalar tag": "value: !custom text\n",
  "custom collection tag": "value: !custom {}\n",
  "non-JSON standard tag": "value: !!set {a: null}\n",
  anchor: "value: &shared text\n",
  alias: "value: *missing\n",
  "merge key": "value: {<<: {a: b}}\n",
  "quoted merge key": 'value: {"<<": {a: b}}\n',
  "multiple documents": "name: first\n---\nname: second\n",
  "empty document": "",
  "sequence document": "- value\n",
  "scalar document": "value\n",
  "YAML 1.1 directive": "%YAML 1.1\n---\nname: Example\n",
  "invalid explicit float": "value: !!float 2x\n",
})) {
  test(`rejects YAML ${label}`, () => {
    assert.throws(() => parseCheck(source));
  });
}

test("allows the standard non-specific YAML tag", () => {
  assert.deepEqual(parseCheck("name: ! Plain\n"), { name: "Plain" });
});

test("validates the exact five linked definition files, names, and schema modelines", async () => {
  const directory = new URL("../../../.antiburn/checks/", import.meta.url);
  const readme = await readFile(
    new URL("../README.md", import.meta.url),
    "utf8",
  );
  const entries = (await readdir(directory, { withFileTypes: true })).filter(
    (entry) => /\.ya?ml$/.test(entry.name),
  );
  assert.ok(
    entries.every((entry) => entry.isFile()),
    "Canonical files must not be symlinks.",
  );
  const files = entries.map((entry) => entry.name);
  const names = {
    "over-exploring.yml": "Over-exploring",
    "scope-creep.yml": "Scope creep",
    "ignored-instructions.yml": "Ignored instructions",
    "skill-opportunities.yml": "Skill opportunities",
    "fast-mode-value.yml": "Fast mode value",
  };
  assert.deepEqual(files.sort(), Object.keys(names).sort());
  for (const file of files) {
    const source = await readFile(new URL(file, directory), "utf8");
    assert.match(
      source,
      /^# yaml-language-server: \$schema=\.\.\/\.\.\/docs\/burn-check\/schemas\/burn-check\.schema\.json/m,
    );
    assert.ok(
      readme.includes(`../../.antiburn/checks/${file}`),
      `${file} is linked from README`,
    );
    assert.doesNotMatch(source, /^\s*turn_end\s*:/m, file);
    const schema = source.match(
      /^# yaml-language-server: \$schema=(.+)$/m,
    )?.[1];
    assert.equal(
      resolve(fileURLToPath(directory), schema),
      resolve(docsRoot, "schemas/burn-check.schema.json"),
      file,
    );
    const check = validate(source);
    assert.equal(check.name, names[file], file);
    assert.ok("tool_result" in check.on, file);
    assert.ok(
      check.actions.every((action) => action.type === "status"),
      file,
    );
    const failures = check.actions.filter(
      (action) => action.input.result === "failure",
    );
    assert.equal(failures.length, file === "ignored-instructions.yml" ? 2 : 1);
    for (const action of failures) {
      assert.match(
        action.if,
        /^decisions\.review\.outputs\.([a-z][a-z0-9_]*) == '([a-z][a-z0-9_]*)' && decisions\.review\.details\.\1\.probabilities\.\2 >= 0\.95$/,
        file,
      );
    }
    if (file !== "ignored-instructions.yml") {
      assert.equal(
        check.actions.find((action) => action.input.result === "success")?.if,
        `!(${failures[0].if})`,
        file,
      );
    }
    for (const decision of Object.values(check.decisions)) {
      assert.equal("model" in decision, false, file);
      assert.deepEqual(
        decision.fit,
        {
          ...(file === "ignored-instructions.yml"
            ? {
                "current_global_scope.skills[*].description": "truncate",
                "current_repo_scope.skills[*].description": "truncate",
              }
            : {}),
          "activity[*].tool.result.content": "truncate",
        },
        file,
      );
    }
    if (file === "ignored-instructions.yml") {
      assert.deepEqual(check.on.window, { size: 20, min: 3 });
      assert.equal(
        check.actions.find((action) => action.input.result === "success")?.if,
        "decisions.review.outputs.decision == 'no_conflict' && decisions.review.details.decision.probabilities.no_conflict >= 0.95",
      );
    }
  }
});

test("documentation does not keep a second YAML definition copy", async () => {
  const entries = await readdir(new URL("../", import.meta.url));
  if (entries.includes("examples")) {
    const files = await readdir(new URL("../examples/", import.meta.url));
    assert.deepEqual(
      files.filter((file) => /\.ya?ml$/i.test(file)),
      [],
    );
  }
});

test("Burn Checks Markdown links, inbound references, and heading anchors are valid", async () => {
  const files = [
    ...(await markdownFiles()),
    resolve(docsRoot, "../../AGENTS.md"),
    resolve(docsRoot, "../session-coverage.md"),
    resolve(docsRoot, "../check-coverage.md"),
    resolve(docsRoot, "../main-window-navigation.md"),
  ];
  for (const file of files) {
    const source = await readFile(file, "utf8");
    const headings = markdownHeadings(source);
    for (const [, destination] of source.matchAll(
      /(?<!!)\[[^\]]+\]\(([^)]+)\)/g,
    )) {
      if (/^(?:[a-z][a-z\d+.-]*:|\/\/)/i.test(destination)) {
        continue;
      }
      const [pathname, rawFragment] = destination.split("#", 2);
      const target = pathname
        ? resolve(dirname(file), decodeURIComponent(pathname))
        : file;
      if (!file.startsWith(docsRoot) && !target.startsWith(docsRoot)) {
        continue;
      }
      const targetSource = await readFile(target, "utf8");
      if (rawFragment) {
        assert.ok(
          markdownHeadings(targetSource).has(decodeURIComponent(rawFragment)),
          `${relative(docsRoot, file)} links to missing heading #${rawFragment}`,
        );
      }
    }
    assert.ok(headings.size > 0, relative(docsRoot, file));
  }
});

test("schema accepts cycle triggers and draft-only message actions", () => {
  const check = base();
  check.on = { turn_end: { status: ["completed"] } };
  check.decisions.review.input = { cycle: "${{ turn }}" };
  check.decisions.review.output.verdict = {
    type: "score",
    question: "How useful is the evidence?",
    criteria: ["Weak", "Strong"],
  };
  check.decisions.extra = {
    input: {
      records: {
        map: "${{ turn.messages }}",
        as: "item",
        if: "item.kind == 'tool_result'",
        value: "${{ item.tool.result }}",
      },
    },
    output: { useful: { type: "noul", question: "Is it useful?" } },
  };
  check.actions.push({
    type: "message",
    input: {
      message: "Review this result: ${{ decisions.review.outputs.verdict }}",
    },
  });
  assert.doesNotThrow(() => validateObject(check));
});

test("validator states its structural boundary without claiming evidence proof", () => {
  assert.match(validationBoundary, /v1-alpha/);
  assert.match(validationBoundary, /no backward compatibility promise/);
  assert.match(validationBoundary, /Beta and stable scope are not set/);
  assert.match(validationBoundary, /not runtime support, native source proof/);
  assert.match(validationBoundary, /Message actions remain unsupported/);
});

test("schema accepts each typed trigger and same-family alternatives", () => {
  for (const on of [
    { user_message: null },
    { assistant_message: { models: ["provider/model"], fast_mode: false } },
    { tool_call: { tools: ["write"], paths: ["src/**"] } },
    { tool_result: { status: ["completed", "error", "cancelled", "unknown"] } },
    { turn_end: { status: ["completed", "cancelled", "error"] } },
    { session_start: { reason: ["new", "resumed", "attached"] } },
    { session_idle: { fast_mode: true } },
    {
      user_message: null,
      assistant_message: {},
      tool_call: { tools: ["shell"] },
      tool_result: { status: ["error"] },
      window: { size: 5, min: 2 },
    },
    { session_start: null, session_idle: {}, cooldown: "0s" },
  ]) {
    assert.doesNotThrow(() =>
      validateObject({
        name: "Typed event structure",
        on,
        actions: [{ type: "status", input: { result: "success" } }],
      }),
    );
  }
});

test("schema rejects filters outside their event and non-message windows", () => {
  for (const on of [
    { user_message: { tools: ["shell"] } },
    { assistant_message: { paths: ["src/**"] } },
    { tool_call: { status: ["completed"] } },
    { turn_end: { status: ["unknown"] } },
    { session_start: { reason: ["unknown"] } },
    { session_idle: { reason: ["attached"] } },
    { session_start: null, window: 2 },
    { session_idle: null, tool_result: null },
  ]) {
    assert.throws(() => validateObject({ ...base(), on }));
  }
});

test("typed blocks allow mixed questions and keep the eight-question limit per block", () => {
  const check = base();
  const score = {
    type: "score",
    question: "How useful is the evidence?",
    criteria: ["Weak.", "Strong."],
  };
  const noul = { type: "noul", question: "Does this apply?" };
  check.decisions.review.output = {
    verdict: choice,
    severity: score,
    applies: noul,
  };
  assert.doesNotThrow(() => validateObject(check));
  for (const count of [8, 9]) {
    check.decisions.review.output = Object.fromEntries(
      Array.from({ length: count }, (_, index) => [`answer_${index}`, noul]),
    );
    if (count === 8) {
      assert.doesNotThrow(() => validateObject(check));
    } else {
      assert.throws(() => validateObject(check));
    }
  }
  check.decisions = Object.fromEntries(
    Array.from({ length: 9 }, (_, index) => [
      `block_${index}`,
      { input: { value: true }, output: { applies: noul } },
    ]),
  );
  assert.doesNotThrow(() => validateObject(check));
});

test("typed criteria require fixed strings and Noul has no authored criteria", () => {
  for (const output of [
    { ...choice, criteria: { yes: true, no: "Not useful." } },
    { type: "score", question: "How useful?", criteria: ["Weak.", 1] },
    { type: "score", question: "How useful?", criteria: ["Weak."] },
    { type: "noul", question: "Is this useful?", criteria: ["No.", "Yes."] },
  ]) {
    const check = base();
    check.decisions.review.output = { answer: output };
    assert.throws(() => validateObject(check));
  }
});

test("the complete typed authoring example is structurally valid", async () => {
  const source = await readFile(
    new URL("../authoring.md", import.meta.url),
    "utf8",
  );
  const example = source.match(
    /```yaml\n(name: Recorded result review\n[\s\S]*?)\n```/,
  )?.[1];
  assert.ok(example, "The typed authoring example must remain complete.");
  assert.doesNotThrow(() => validate(example));
});

test("the mixed message authoring example is structurally valid", async () => {
  const source = await readFile(
    new URL("../authoring.md", import.meta.url),
    "utf8",
  );
  const example = source.match(
    /```yaml\n(name: Selected message window\n[\s\S]*?)\n```/,
  )?.[1];
  assert.ok(example, "The mixed message example must remain complete.");
  assert.doesNotThrow(() => validate(example));
});

test("the local authoring example omits model decisions and is structurally valid", async () => {
  const source = await readFile(
    new URL("../authoring.md", import.meta.url),
    "utf8",
  );
  const example = source.match(
    /```yaml\n(name: Recorded tool error\n[\s\S]*?)\n```/,
  )?.[1];
  assert.ok(example, "The local authoring example must remain complete.");
  const check = validate(example);
  assert.equal("decisions" in check, false);
});

test("accepts structural expressions without claiming to parse them", () => {
  const check = base();
  check.if = "${{ unknown.reference + }}";
  check.on = { tool_result: null, window: { size: 1, min: 2 } };
  check.actions[0].input.result = "${{ }}";
  assert.doesNotThrow(() => validateObject(check));
});

test("rejects unknown keys, YAML version fields, and mixed trigger families", () => {
  for (const mutate of [
    (check) => {
      check.extra = true;
    },
    (check) => {
      check.version = 1;
    },
    (check) => {
      check.$schema = "schemas/burn-check.schema.json";
    },
    (check) => {
      check.on = { tool_result: null, turn_end: null };
    },
    (check) => {
      check.actions[0].input.success_if = true;
    },
  ]) {
    const check = base();
    mutate(check);
    assert.throws(() => validateObject(check));
  }
});

test("status results accept only literals or complete expression wrappers", () => {
  for (const result of ["success", "failure", "${{ 'success' }}"]) {
    const check = base();
    check.actions[0].input.result = result;
    assert.doesNotThrow(() => validateObject(check));
  }
  for (const result of [
    true,
    null,
    "Success",
    " success",
    "success suffix",
    "status == true",
  ]) {
    const check = base();
    check.actions[0].input.result = result;
    assert.throws(() => validateObject(check));
  }
});

test("rejects invalid event, window, observed model, and action shapes", () => {
  const mutations = [
    (check) => {
      check.on = { unknown_event: null };
    },
    (check) => {
      check.on = { tool_result: null, window: 0 };
    },
    (check) => {
      check.on = { turn_end: null, window: 5 };
    },
    (check) => {
      check.on.tool_result = { models: ["model"] };
    },
    (check) => {
      check.on.tool_result = { models: ["provider/model name"] };
    },
    (check) => {
      check.actions = [];
    },
    (check) => {
      check.actions[0].input = {};
    },
    (check) => {
      check.actions = [{ type: "message", input: {} }];
    },
  ];
  for (const mutate of mutations) {
    const check = base();
    mutate(check);
    assert.throws(() => validateObject(check));
  }
});

test("Settings owns routing while model filters remain observed source facts", () => {
  assert.doesNotThrow(() => validateObject(base()));
  for (const model of ["typesafe/jev-1.13.0", "other/model", null, {}]) {
    const check = base();
    check.decisions.review.model = model;
    assert.throws(() => validateObject(check), /additional properties/);
  }
  for (const event of ["tool_result", "turn_end", "session_start"]) {
    const check = base();
    check.on = { [event]: { models: ["provider/observed-model"] } };
    assert.doesNotThrow(() => validateObject(check));
  }
  const check = base();
  check.if = "session.model == 'provider/observed-model'";
  check.decisions.review.input = {
    message_model: "${{ message.model }}",
    turn_model: "${{ turn.model }}",
    session_model: "${{ session.model }}",
  };
  assert.doesNotThrow(() => validateObject(check));
});

test("Choice schema retains structural rules without provider criteria limits", () => {
  for (const count of [2, 26, 27, 100]) {
    const check = base();
    check.decisions.review.output.verdict = {
      ...choice,
      criteria: Object.fromEntries(
        Array.from({ length: count }, (_, index) => [
          `label_${index}`,
          "Synthetic label.",
        ]),
      ),
    };
    assert.doesNotThrow(() => validateObject(check));
  }
  for (const criteria of [
    {},
    { only: "One label." },
    { yes: "", no: "Nonblank." },
    { yes: "Valid.", Invalid: "Invalid identifier." },
  ]) {
    const check = base();
    check.decisions.review.output.verdict = { ...choice, criteria };
    assert.throws(() => validateObject(check));
  }
});

test("map aliases and input values are structurally validated without semantic compilation", () => {
  const check = base();
  check.decisions.review.input.records = {
    map: "${{ window.messages }}",
    as: "item",
    if: "item.tool.result.status == 'error'",
    value: { result: "${{ item.tool.result }}" },
  };
  assert.doesNotThrow(() => validateObject(check));
  check.decisions.review.input.records = {
    map: "${{ window.messages }}",
    value: "item",
  };
  assert.throws(() => validateObject(check));
});
