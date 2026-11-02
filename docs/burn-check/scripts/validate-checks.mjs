import { readFile, readdir } from "node:fs/promises";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";
import Ajv2020 from "ajv/dist/2020.js";
import { isAlias, isMap, isScalar, parseAllDocuments, visit } from "yaml";

export const validationBoundary =
  "Burn Checks v1-alpha is experimental and not yet standardized. Syntax, semantics, and capabilities can change, including breaking changes, with no backward compatibility promise. Beta and stable scope are not set. Missing features are unassigned future work, not usable today. Validation covers YAML restrictions and schema structure only, not runtime support, native source proof, or provider capability. The compiler and runtime check expression syntax (including blank expressions), event-scoped roots and filters, closed-record access, frozen evidence, source and evaluation bounds, glob syntax, aliases, window min <= size, and fit rules. Message alternatives share one rolling window; turn and session events have no window. Current session selection cannot fill past attribution. The desktop emits session_start reason attached only. The 120-second idle policy is an observation, not measured activity. Message actions remain unsupported even behind false conditions. Status result expressions must return exactly success or failure, case-sensitive, without Boolean coercion.";

export function parseCheck(source) {
  const documents = parseAllDocuments(source, {
    version: "1.2",
    schema: "core",
    // YAML 1.2.2 allows integer-form !!float values. Keep implicit core tags unchanged.
    customTags: [
      {
        tag: "tag:yaml.org,2002:float",
        default: false,
        resolve(value, onError) {
          if (
            !/^[-+]?(?:[0-9]+(?:\.[0-9]*)?|\.[0-9]+)(?:[eE][0-9]+)?(?![\s\S])/.test(
              value,
            )
          ) {
            onError("Expected a finite decimal float.");
          }
          return Number(value);
        },
      },
    ],
    uniqueKeys: true,
    merge: false,
  });
  if (documents.length !== 1) {
    throw new Error("Expected exactly one YAML document.");
  }
  const [document] = documents;
  const issues = [...document.errors, ...document.warnings];
  if (issues.length) {
    throw new Error(issues.map((issue) => issue.message).join("\n"));
  }
  if (document.directives.yaml.version !== "1.2") {
    throw new Error("Expected YAML 1.2.");
  }
  if (!isMap(document.contents)) {
    throw new Error("Expected a mapping document.");
  }
  visit(document, {
    Node(_key, node) {
      if (isAlias(node) || node.anchor) {
        throw new Error("Anchors and aliases are not allowed.");
      }
      if (
        node.tag &&
        ![
          "!",
          "tag:yaml.org,2002:str",
          "tag:yaml.org,2002:null",
          "tag:yaml.org,2002:bool",
          "tag:yaml.org,2002:int",
          "tag:yaml.org,2002:float",
          "tag:yaml.org,2002:map",
          "tag:yaml.org,2002:seq",
        ].includes(node.tag)
      ) {
        throw new Error("Custom tags are not allowed.");
      }
      if (isScalar(node)) {
        const value = node.value;
        if (typeof value === "number" && !Number.isFinite(value)) {
          throw new Error("Nonfinite numbers are not allowed.");
        }
        if (
          value !== null &&
          !["string", "number", "boolean"].includes(typeof value)
        ) {
          throw new Error("Expected a JSON-compatible scalar.");
        }
      }
      if (isMap(node)) {
        for (const pair of node.items) {
          if (!isScalar(pair.key) || typeof pair.key.value !== "string") {
            throw new Error("Mapping keys must be strings.");
          }
          if (pair.key.value === "<<") {
            throw new Error("Merge keys are not allowed.");
          }
        }
      }
    },
  });
  return document.toJS();
}

export async function createValidator() {
  const schema = JSON.parse(
    await readFile(
      new URL("../schemas/burn-check.schema.json", import.meta.url),
      "utf8",
    ),
  );
  const ajv = new Ajv2020({ allErrors: true, strict: true });
  if (!ajv.validateSchema(schema)) {
    throw new Error(ajv.errorsText(ajv.errors));
  }
  const validate = ajv.compile(schema);
  return (source) => {
    const check = parseCheck(source);
    if (!validate(check)) {
      throw new Error(ajv.errorsText(validate.errors, { separator: "\n" }));
    }
    return check;
  };
}

async function main() {
  console.log(validationBoundary);
  const validate = await createValidator();
  console.log("Schema is valid (JSON Schema Draft 2020-12).");
  const definitions = new URL("../../../.antiburn/checks/", import.meta.url);
  const supplied = process.argv.slice(2);
  const files = supplied.length
    ? supplied.map((file) => resolve(file))
    : (await readdir(definitions))
        .filter((file) => /\.ya?ml$/i.test(file))
        .sort()
        .map((file) => fileURLToPath(new URL(file, definitions)));
  if (!files.length) {
    throw new Error("No Burn Check definitions found.");
  }
  let failures = 0;
  for (const file of files) {
    try {
      validate(await readFile(file, "utf8"));
      console.log(`PASS ${file}`);
    } catch (error) {
      failures++;
      console.error(`FAIL ${file}\n${error.message}`);
    }
  }
  console.log(`${files.length - failures}/${files.length} checks passed.`);
  if (failures) {
    process.exitCode = 1;
  }
}

if (
  process.argv[1] &&
  resolve(process.argv[1]) === fileURLToPath(import.meta.url)
) {
  main().catch((error) => {
    console.error(error.message);
    process.exitCode = 1;
  });
}
