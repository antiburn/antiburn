import assert from "node:assert/strict";
import test from "node:test";

import { checkDesignDrift } from "./check-design-drift.mjs";

const CSS = `@theme {
  --color-surface: var(--color-bg-primary);
  --radius-popover: 10px;
}

:root {
  --color-bg-primary: rgb(255 255 255);

}

@media (prefers-color-scheme: dark) {
  :root {
    --color-bg-primary: rgb(30 30 30);
  }
}

:root[data-theme="light"] {
  --color-bg-primary: rgb(255 255 255);
}

:root[data-theme="dark"] {
  --color-bg-primary: rgb(30 30 30);
}

html,
body {
  line-height: 1.4;
}

.type-body {
  font-size: 13px;
  font-weight: 400;
  letter-spacing: -0.08px;
}

`;

function fixture({
  css = CSS,
  doc = "",
  extra = {},
  popover = "const CORNER_RADIUS: f64 = 10.0;",
  nudge = "const NUDGE_CORNER_RADIUS: f64 = 10.0;",
} = {}) {
  const files = {
    "design.md": doc,
    "src/styles/tokens.css": css,
    "src-tauri/src/popover.rs": popover,
    "src-tauri/crates/nudge/src/window.rs": nudge,
    ...extra,
  };
  return {
    read: (path) => {
      assert.ok(path in files, `Unexpected read: ${path}`);
      return files[path];
    },
    exists: (path) => path in files && files[path] !== undefined,
    listStylesheets: () =>
      Object.keys(files).filter((path) => path.endsWith(".css")),
  };
}

function assertFails(failures, fragment) {
  assert.equal(
    failures.filter((failure) => failure.includes(fragment)).length,
    1,
    `Expected one failure containing ${fragment}, got ${JSON.stringify(failures)}`,
  );
}

test("checks CSS and native code without a YAML catalogue or source manifest", () => {
  assert.deepEqual(checkDesignDrift(fixture()), []);
});

test("discovers semantic tokens in new feature stylesheets", () => {
  const extra = {
    "src/components/chart.css":
      "@theme { --color-chart: var(--color-chart-val); }",
  };
  const failures = checkDesignDrift(fixture({ extra }));
  assertFails(failures, "--color-chart-val is not set in the light palette");
  assertFails(failures, "--color-chart-val is not set in the dark palette");
});

test("rejects an empty token scan", () => {
  assertFails(
    checkDesignDrift(fixture({ css: "" })),
    "No semantic color aliases",
  );
});

test("allows coordinated palette changes without a prose update", () => {
  const css = CSS.replaceAll("rgb(255 255 255)", "rgb(240 240 240)");
  assert.deepEqual(checkDesignDrift(fixture({ css })), []);
});

test("reports missing explicit and system theme values", () => {
  const css = CSS.replaceAll("--color-bg-primary:", "--color-other:");
  const failures = checkDesignDrift(fixture({ css }));
  for (const theme of ["light", "dark"]) {
    assertFails(failures, `is not set in the ${theme} palette`);
    assertFails(failures, `is unset in the ${theme} system palette`);
  }
});

test("compares each explicit theme with its system palette", () => {
  for (const theme of ["light", "dark"]) {
    const original = theme === "light" ? "rgb(255 255 255)" : "rgb(30 30 30)";
    const css = CSS.replace(
      `:root[data-theme="${theme}"] {\n  --color-bg-primary: ${original};`,
      `:root[data-theme="${theme}"] {\n  --color-bg-primary: rgb(9 9 9);`,
    );
    const failures = checkDesignDrift(fixture({ css }));
    assertFails(failures, `\`surface\` ${theme} = rgb(9 9 9)`);
    assert.equal(failures.length, 1);
  }
});

test("reads a palette with a shared root selector", () => {
  const css = CSS.replace(
    ':root[data-theme="light"] {',
    ':root,\n:root[data-theme="light"] {',
  );
  assert.deepEqual(checkDesignDrift(fixture({ css })), []);
});

test("ignores selectors in comments and nested accessibility overrides", () => {
  const css = `${CSS}
/* :root[data-theme="dark"] { --color-bg-primary: rgb(9 9 9); } */
@media (prefers-reduced-transparency: reduce) and (prefers-color-scheme: dark) {
  :root { --color-bg-primary: rgb(9 9 9); }
  :root[data-theme="dark"] { --color-bg-primary: rgb(9 9 9); }
}
@supports (color: -apple-system-label) {
  :root { --color-bg-primary: -apple-system-label; }
}
`;
  assert.deepEqual(checkDesignDrift(fixture({ css })), []);
});

test("requires a static fallback for live system colors", () => {
  const css = CSS.replaceAll("rgb(255 255 255)", "-apple-system-label");
  assertFails(
    checkDesignDrift(fixture({ css })),
    "is not set in the light palette",
  );
});

test("compares equivalent number spellings equally", () => {
  const css = CSS.replace("rgb(255 255 255)", "rgb(255.0 255.00 255)");
  assert.deepEqual(checkDesignDrift(fixture({ css })), []);
});

test("requires a shared unitless base line height", () => {
  const failures = checkDesignDrift(
    fixture({ css: CSS.replace("line-height: 1.4;", "") }),
  );
  assertFails(failures, "html has no shared unitless line-height");
  assertFails(failures, "body has no shared unitless line-height");
});

test("scoped selectors cannot define or override global palette values", () => {
  const scoped = `
:root[data-theme="dark"] .card { --color-bg-primary: rgb(9 9 9); }
:root .card { --color-bg-primary: rgb(9 9 9); }
`;
  assert.deepEqual(checkDesignDrift(fixture({ css: CSS + scoped })), []);
  const css = CSS.replaceAll("--color-bg-primary:", "--color-other:") + scoped;
  const failures = checkDesignDrift(fixture({ css }));
  assertFails(failures, "is not set in the dark palette");
  assertFails(failures, "is unset in the light system palette");
});

test("type roles inherit line height", () => {
  const css = CSS.replace(".type-body {", ".type-body { line-height: 1.2;");
  assertFails(
    checkDesignDrift(fixture({ css })),
    ".type-body sets its own line-height",
  );
});

test("compares both native corners directly with the CSS token", () => {
  const failures = checkDesignDrift(
    fixture({
      css: CSS.replace("--radius-popover: 10px", "--radius-popover: 12px"),
    }),
  );
  assertFails(failures, "--radius-popover 12px != CORNER_RADIUS 10.0");
  assertFails(failures, "--radius-popover 12px != NUDGE_CORNER_RADIUS 10.0");
});

test("reports native corner changes", () => {
  const failures = checkDesignDrift(
    fixture({
      popover: "const CORNER_RADIUS: f64 = 12.0;",
      nudge: "const NUDGE_CORNER_RADIUS: f64 = 11.0;",
    }),
  );
  assertFails(failures, "--radius-popover 10px != CORNER_RADIUS 12.0");
  assertFails(failures, "--radius-popover 10px != NUDGE_CORNER_RADIUS 11.0");
});

test("reports a missing CSS radius", () => {
  assertFails(
    checkDesignDrift(
      fixture({ css: CSS.replace("--radius-popover: 10px;", "") }),
    ),
    "--radius-popover is missing",
  );
});

test("reports missing native sources and constants", () => {
  const failures = checkDesignDrift(
    fixture({
      nudge: "// const NUDGE_CORNER_RADIUS: f64 = 10.0;",
      extra: { "src-tauri/src/popover.rs": undefined },
    }),
  );
  assertFails(failures, "src-tauri/src/popover.rs not found");
  assertFails(failures, "declares no NUDGE_CORNER_RADIUS");
});

for (const selector of [
  ".type-body, .compact",
  ".type-body:hover",
  ".compact, .type-body:focus",
]) {
  test(`rejects line-height overrides in ${selector}`, () => {
    const css =
      CSS + `@media (min-width: 1px) { ${selector} { line-height: 1.2; } }`;
    assertFails(
      checkDesignDrift(fixture({ css })),
      ".type-body sets its own line-height",
    );
  });
}

test("custom properties do not count as line-height declarations", () => {
  const css = CSS + ".type-body { --line-height: 1.2; }";
  assert.deepEqual(checkDesignDrift(fixture({ css })), []);
  const failures = checkDesignDrift(
    fixture({ css: CSS.replace("line-height: 1.4;", "--line-height: 1.4;") }),
  );
  assertFails(failures, "html has no shared unitless line-height");
  assertFails(failures, "body has no shared unitless line-height");
});

function exceptionDoc(rows) {
  return `## System palette exceptions\n\n| Token | Theme | System value | Rationale |\n| ----- | ----- | ------------ | --------- |\n${rows}\n`;
}
const DIFFERENT_SYSTEM = CSS.replace("rgb(255 255 255)", "rgb(240 240 240)");

test("allows a documented System palette difference", () => {
  const doc = exceptionDoc(
    "| surface | light | rgb(240 240 240) | Native material needs different ink. |",
  );
  assert.deepEqual(
    checkDesignDrift(fixture({ css: DIFFERENT_SYSTEM, doc })),
    [],
  );
});

test("rejects stale, redundant, and unknown System palette exceptions", () => {
  const row = "| surface | light | rgb(241 241 241) | Native material. |";
  assertFails(
    checkDesignDrift(
      fixture({ css: DIFFERENT_SYSTEM, doc: exceptionDoc(row) }),
    ),
    "expected rgb(240 240 240)",
  );
  assertFails(
    checkDesignDrift(fixture({ doc: exceptionDoc(row) })),
    "repeats the explicit value",
  );
  assertFails(
    checkDesignDrift(
      fixture({ doc: exceptionDoc(row.replace("surface", "unknown")) }),
    ),
    "unknown token",
  );
});

test("rejects duplicate or malformed exception rows", () => {
  const row = "| surface | light | rgb(240 240 240) | Native material. |";
  assertFails(
    checkDesignDrift(
      fixture({ css: DIFFERENT_SYSTEM, doc: exceptionDoc(`${row}\n${row}`) }),
    ),
    "Duplicate System palette exception",
  );
  assertFails(
    checkDesignDrift(
      fixture({ doc: exceptionDoc(row.replace("light", "dakr")) }),
    ),
    "Invalid System palette exception",
  );
});

test("checks documented source paths and CSS classes", () => {
  const doc =
    "src/styles/tokens.css .type-body ui-monospace ui-sans-serif ui-serif ui-rounded";
  assert.deepEqual(checkDesignDrift(fixture({ doc })), []);
  const failures = checkDesignDrift(
    fixture({ doc: "src/missing.tsx ui-missing type-missing" }),
  );
  assertFails(failures, "referenced path does not exist: src/missing.tsx");
  assertFails(failures, "referenced class not found in the CSS: ui-missing");
  assertFails(failures, "referenced class not found in the CSS: type-missing");
});
