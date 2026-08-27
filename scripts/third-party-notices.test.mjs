import assert from "node:assert/strict";
import test from "node:test";

import {
  frontendLegalTexts,
  frontendPackages,
  generatedSection,
  legalBodies,
  replaceGeneratedSection,
  rustPackages,
} from "./third-party-notices.mjs";

test("formats dependency inventories without machine-specific paths", () => {
  const frontend = frontendPackages(
    {
      MIT: [
        {
          name: "example-web",
          versions: ["2.0.0"],
          paths: ["/private/example-web"],
          license: "MIT",
          homepage: "https://example.com/web",
        },
      ],
    },
    () => ["MIT license text"],
  );
  const report = {
    crates: [
      {
        package: { name: "app", version: "1.0.0", source: null },
        license: "MIT",
      },
      {
        package: {
          name: "example-crate",
          version: "3.0.0",
          source: "registry+example",
          repository: "https://example.com/crate",
        },
        license: "MPL-2.0",
      },
      {
        package: { name: "test-only", version: "1.0.0", source: null },
        license: "Apache-2.0",
      },
    ],
    licenses: [
      {
        text: "MPL license text",
        used_by: [
          {
            crate: {
              name: "example-crate",
              version: "3.0.0",
              source: "registry+example",
            },
          },
          {
            crate: { name: "app", version: "1.0.0", source: null },
          },
        ],
      },
    ],
  };
  const rust = rustPackages(report);

  const result = generatedSection(
    frontend,
    rust,
    legalBodies(frontend, report),
  );
  assert.match(
    result,
    /example-web@2\.0\.0 \| MIT \| https:\/\/example\.com\/web/,
  );
  assert.match(result, /example-crate@3\.0\.0 \| MPL-2\.0/);
  assert.match(result, /MIT license text/);
  assert.match(result, /MPL license text/);
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

test("rejects a frontend package without legal text", () => {
  assert.throws(
    () => frontendLegalTexts("missing-package@1.0.0", []),
    /has no distributable frontend license text/,
  );
});

test("deduplicates identical legal texts and lists every package", () => {
  const frontend = [
    { name: "alpha", version: "1.0.0", legalTexts: ["same text"] },
    { name: "beta", version: "2.0.0", legalTexts: ["same text\n"] },
  ];

  assert.deepEqual(legalBodies(frontend, { licenses: [] }), [
    {
      text: "same text\n",
      packages: ["alpha@1.0.0", "beta@2.0.0"],
    },
  ]);
});
