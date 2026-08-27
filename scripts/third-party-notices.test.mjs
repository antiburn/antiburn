import assert from "node:assert/strict";
import test from "node:test";

import {
  frontendPackages,
  generatedSection,
  replaceGeneratedSection,
  rustPackages,
} from "./third-party-notices.mjs";

test("formats dependency inventories without machine-specific paths", () => {
  const frontend = frontendPackages({
    MIT: [
      {
        name: "example-web",
        versions: ["2.0.0"],
        paths: ["/private/example-web"],
        license: "MIT",
        homepage: "https://example.com/web",
      },
    ],
  });
  const rust = rustPackages({
    workspace_members: ["app"],
    packages: [
      {
        id: "app",
        name: "app",
        version: "1.0.0",
        source: null,
        license: "MIT",
      },
      {
        id: "crate",
        name: "example-crate",
        version: "3.0.0",
        source: "registry+example",
        license: "MPL-2.0",
        repository: "https://example.com/crate",
      },
      {
        id: "test-only",
        name: "test-only",
        version: "1.0.0",
        source: "registry+example",
        license: "MIT",
      },
    ],
    resolve: {
      nodes: [
        {
          id: "app",
          deps: [
            { pkg: "crate", dep_kinds: [{ kind: null }] },
            { pkg: "test-only", dep_kinds: [{ kind: "dev" }] },
          ],
        },
        { id: "crate", deps: [] },
        { id: "test-only", deps: [] },
      ],
    },
  });

  const result = generatedSection(frontend, rust);
  assert.match(
    result,
    /example-web@2\.0\.0 \| MIT \| https:\/\/example\.com\/web/,
  );
  assert.match(result, /example-crate@3\.0\.0 \| MPL-2\.0/);
  assert.doesNotMatch(result, /private|test-only/);
});

test("replaces only the generated dependency section", () => {
  const current = [
    "hand-maintained",
    "----- BEGIN GENERATED DEPENDENCY NOTICES -----",
    "old",
    "----- END GENERATED DEPENDENCY NOTICES -----",
    "footer",
    "",
  ].join("\n");
  const generated = [
    "----- BEGIN GENERATED DEPENDENCY NOTICES -----",
    "new",
    "----- END GENERATED DEPENDENCY NOTICES -----",
    "",
  ].join("\n");

  assert.equal(
    replaceGeneratedSection(current, generated),
    `hand-maintained\n${generated}footer\n`,
  );
});

test("rejects an unreviewed frontend license", () => {
  assert.throws(
    () =>
      frontendPackages({
        Unknown: [
          { name: "new-package", versions: ["1.0.0"], license: "Unknown" },
        ],
      }),
    /unreviewed frontend license Unknown/,
  );
});

test("uses the reviewed license for the pinned tauri-nspanel revision", () => {
  const packages = rustPackages({
    workspace_members: ["app"],
    packages: [
      {
        id: "app",
        name: "app",
        version: "1.0.0",
        source: null,
        license: "MIT",
      },
      {
        id: "panel",
        name: "tauri-nspanel",
        version: "2.1.0",
        source:
          "git+https://github.com/ahkohd/tauri-nspanel?rev=a3122e894383aa068ec5365a42994e3ac94ba1b6#a3122e894383aa068ec5365a42994e3ac94ba1b6",
        license: null,
      },
    ],
    resolve: {
      nodes: [
        { id: "app", deps: [{ pkg: "panel", dep_kinds: [{ kind: null }] }] },
        { id: "panel", deps: [] },
      ],
    },
  });

  assert.equal(packages[0].license, "MIT OR Apache-2.0");
  assert.match(packages[0].source, /a3122e894383aa068ec5365a42994e3ac94ba1b6$/);
});
