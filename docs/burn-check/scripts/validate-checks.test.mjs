import assert from "node:assert/strict";
import { readFile, readdir } from "node:fs/promises";
import { dirname, extname, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";
import { createValidator, parseCheck } from "./validate-checks.mjs";

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
      model: "typesafe/jev-1.13.0",
      input: { activity: "${{ window.messages }}" },
      output: { verdict: choice },
    },
  },
  actions: [{ type: "status", input: { result: "failure" } }],
});
const validateObject = (value) => validate(JSON.stringify(value));
const docsRoot = fileURLToPath(new URL("../", import.meta.url));

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

test("validates all six linked examples", async () => {
  const directory = new URL("../examples/", import.meta.url);
  const readme = await readFile(
    new URL("../README.md", import.meta.url),
    "utf8",
  );
  const files = (await readdir(directory)).filter((file) =>
    /\.ya?ml$/.test(file),
  );
  assert.equal(files.length, 6);
  for (const file of files) {
    const source = await readFile(new URL(file, directory), "utf8");
    assert.match(
      source,
      /^# yaml-language-server: \$schema=\.\.\/schemas\/burn-check\.schema\.json/m,
    );
    assert.ok(
      readme.includes(`examples/${file}`),
      `${file} is linked from README`,
    );
    assert.doesNotMatch(source, /^\s*turn_end\s*:/m, file);
    assert.doesNotThrow(() => validate(source), file);
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

test("accepts draft forms beyond the supported runtime subset", () => {
  const check = base();
  check.on = { turn_end: { status: ["completed"] } };
  check.decisions.review.output.verdict = {
    type: "score",
    question: "How useful is the evidence?",
    criteria: ["Weak", "Strong"],
  };
  check.decisions.extra = {
    model: "provider/model",
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

test("rejects invalid event, window, model, and action shapes", () => {
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
      check.decisions.review.model = "model";
    },
    (check) => {
      check.decisions.review.model = "provider/model name";
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
