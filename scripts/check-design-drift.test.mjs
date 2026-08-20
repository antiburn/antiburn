// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import assert from 'node:assert/strict';
import test from 'node:test';

import { checkDesignDrift } from './check-design-drift.mjs';

const CONTRACT = `---
version: alpha
name: test
sources:
  - src/styles/tokens.css
colors:
  surface:
    light: "rgb(255 255 255)"
    dark: "rgb(30 30 30)"
fonts:
  sans: "system-ui, sans-serif"
  mono: "ui-monospace, Menlo, monospace"
typography:
  body: { fontSize: 13px, fontWeight: 400, lineHeight: 1.4, letterSpacing: "-0.08px" }
spacing:
  1: 4px
  2: 8px
  base: 4px
sizes:
  --sidebar-width: 220px
rounded:
  small: 4px
  control: 5px
  popover: 10px
  full: 9999px
shadow:
  popover: "0 4px 12px rgb(0 0 0 / 0.15)"
  tooltip: "0 2px 8px rgb(0 0 0 / 0.12)"
  raised: "0 1px 2px rgb(0 0 0 / 0.15)"
motion:
  --duration-fast: 120ms
  --ease-out-quart: cubic-bezier(0.23, 1, 0.32, 1)
  button: "transform 80ms ease-out"
components:
  button:
    className: ui-push-button
    textColor: "{colors.surface}"
---

# Test contract

The type scale lives in src/styles/tokens.css.
`;

const CSS = `@theme {
  --color-surface: var(--color-bg-primary);
  --radius-control: 5px;
  --radius-popover: 10px;
  --radius-small: 4px;
}

:root {
  --space-xs: 4px;
  --space-sm: 8px;
  --sidebar-width: 220px;
  --duration-fast: 120ms;
  --ease-out-quart: cubic-bezier(0.23, 1, 0.32, 1);
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

.ui-push-button {
  height: 22px;
}

.ui-menu {
  box-shadow: 0 4px 12px rgb(0 0 0 / 0.15);
}

.ui-tooltip {
  box-shadow: 0 2px 8px rgb(0 0 0 / 0.12);
}

.ui-switch-thumb {
  box-shadow: 0 1px 2px rgb(0 0 0 / 0.15);
}
`;

/** An in-memory desktop app. Keys are paths relative to apps/desktop. */
function io(files) {
  return {
    read: (rel) => {
      if (!(rel in files)) throw new Error(`missing ${rel}`);
      return files[rel];
    },
    exists: (rel) => rel in files,
    listStylesheets: () => Object.keys(files).filter((f) => f.endsWith('.css')),
  };
}

/** The green fixture, with optional edits to either side of the contract. */
function fixture({ contract = CONTRACT, css = CSS, extra = {} } = {}) {
  return io({ 'design.md': contract, 'src/styles/tokens.css': css, ...extra });
}

function assertFails(failures, fragment) {
  assert.equal(
    failures.filter((f) => f.includes(fragment)).length,
    1,
    `expected one failure containing ${JSON.stringify(fragment)}, got ${JSON.stringify(failures)}`,
  );
}

test('a contract that matches the CSS reports no drift', () => {
  assert.deepEqual(checkDesignDrift(fixture()), []);
});

test('reports an @theme token that the contract does not document', () => {
  const css = CSS.replace(
    '--color-surface: var(--color-bg-primary);',
    '--color-surface: var(--color-bg-primary);\n  --color-label: var(--color-text-primary);',
  );
  assertFails(checkDesignDrift(fixture({ css })), '`label` is not documented in colors');
});

test('reports a documented color that no @theme block registers', () => {
  const contract = CONTRACT.replace(
    '  surface:\n',
    '  ghost:\n    light: "rgb(1 1 1)"\n    dark: "rgb(2 2 2)"\n  surface:\n',
  );
  assertFails(checkDesignDrift(fixture({ contract })), '`ghost` is not registered');
});

test('compares each theme against its own palette', () => {
  const css = CSS.replace('--color-bg-primary: rgb(30 30 30);', '--color-bg-primary: rgb(9 9 9);');
  const failures = checkDesignDrift(fixture({ css }));
  assertFails(failures, 'color `surface` dark = rgb(30 30 30), expected rgb(9 9 9)');
  assert.equal(failures.length, 1, 'the light theme must stay green');
});

test('reads a palette that shares its selector list with a plain :root', () => {
  const css = CSS.replace(':root[data-theme="light"] {', ':root,\n:root[data-theme="light"] {');
  assert.deepEqual(checkDesignDrift(fixture({ css })), []);
});

test('ignores a [data-theme] rule nested inside a media query', () => {
  const css = `${CSS}\n@media (prefers-reduced-transparency: reduce) {\n  :root[data-theme="dark"] {\n    --color-bg-primary: rgb(77 77 77);\n  }\n}\n`;
  assert.deepEqual(checkDesignDrift(fixture({ css })), []);
});

test('ignores a live system token nested inside @supports', () => {
  const css = `${CSS}\n@supports (color: -apple-system-label) {\n  :root[data-theme="light"] {\n    --color-bg-primary: -apple-system-label;\n  }\n}\n`;
  assert.deepEqual(checkDesignDrift(fixture({ css })), []);
});

test('skips a live system token declared at the top level', () => {
  const css = CSS.replace(
    ':root[data-theme="light"] {\n  --color-bg-primary: rgb(255 255 255);',
    ':root[data-theme="light"] {\n  --color-bg-primary: -apple-system-label;',
  );
  assertFails(checkDesignDrift(fixture({ css })), 'is not set in the light palette');
});

test('reports a typography step whose size differs', () => {
  const css = CSS.replace('font-size: 13px;', 'font-size: 15px;');
  assertFails(checkDesignDrift(fixture({ css })), 'type `body` fontSize 13 != 15');
});

test('reports a line height that differs from the base element rule', () => {
  const css = CSS.replace('line-height: 1.4;', 'line-height: 1.6;');
  assertFails(checkDesignDrift(fixture({ css })), 'type `body` lineHeight 1.4 != the base 1.6');
});

test('reports a typography step with no class in the CSS', () => {
  const css = CSS.replace('.type-body {', '.type-other {');
  assertFails(checkDesignDrift(fixture({ css })), '.type-body not found');
});

test('reports a spacing scale that differs from the --space-* vars', () => {
  const css = CSS.replace('--space-sm: 8px;', '--space-sm: 10px;');
  assertFails(checkDesignDrift(fixture({ css })), 'spacing [4,8] != --space-* [4,10]');
});

test('reports a radius that differs', () => {
  const css = CSS.replace('--radius-control: 5px;', '--radius-control: 6px;');
  assertFails(checkDesignDrift(fixture({ css })), 'rounded.control 5px != --radius-control 6px');
});

test('reports a geometry var that differs', () => {
  const css = CSS.replace('--sidebar-width: 220px;', '--sidebar-width: 240px;');
  assertFails(checkDesignDrift(fixture({ css })), 'sizes --sidebar-width = 220px, expected 240px');
});

test('reports a motion duration that differs', () => {
  const css = CSS.replace('--duration-fast: 120ms;', '--duration-fast: 150ms;');
  assertFails(checkDesignDrift(fixture({ css })), 'motion --duration-fast = 120ms, expected 150ms');
});

test('reports a motion easing that differs', () => {
  const css = CSS.replace(
    '--ease-out-quart: cubic-bezier(0.23, 1, 0.32, 1);',
    '--ease-out-quart: cubic-bezier(0.4, 0, 0.2, 1);',
  );
  assertFails(checkDesignDrift(fixture({ css })), 'motion --ease-out-quart');
});

test('ignores a motion recipe that names no custom property', () => {
  assert.ok(CONTRACT.includes('button: "transform 80ms ease-out"'), 'fixture must hold a recipe');
  assert.deepEqual(checkDesignDrift(fixture()), []);
});

test('reports a shadow that differs from the selector that carries it', () => {
  const css = CSS.replace('box-shadow: 0 2px 8px rgb(0 0 0 / 0.12);', 'box-shadow: none;');
  assertFails(checkDesignDrift(fixture({ css })), 'shadow.tooltip does not match');
});

test('reports a shadow selector that the CSS does not define', () => {
  const css = CSS.replace('.ui-switch-thumb {', '.ui-switch-knob {');
  assertFails(checkDesignDrift(fixture({ css })), '.ui-switch-thumb not found');
});

test('reports a prose path that does not exist', () => {
  const contract = CONTRACT.replace('src/styles/tokens.css.\n', 'src/styles/gone.css.\n');
  assertFails(checkDesignDrift(fixture({ contract })), 'referenced path does not exist');
});

test('reports a prose class that the CSS does not define', () => {
  const contract = CONTRACT.replace('className: ui-push-button', 'className: ui-ghost-button');
  assertFails(checkDesignDrift(fixture({ contract })), 'ui-ghost-button');
});

test('accepts a generic font family that shares the ui- prefix', () => {
  assert.ok(CONTRACT.includes('ui-monospace'), 'the fixture must exercise the font stack');
  assert.deepEqual(checkDesignDrift(fixture()), []);
});

test('reports an interpolation that does not resolve', () => {
  const contract = CONTRACT.replace('{colors.surface}', '{colors.nowhere}');
  assertFails(checkDesignDrift(fixture({ contract })), 'unresolved interpolation {colors.nowhere}');
});

test('reports a stylesheet that the source list omits', () => {
  const extra = { 'src/styles/extra.css': '.thing { color: red; }\n' };
  assertFails(
    checkDesignDrift(fixture({ extra })),
    'stylesheet `src/styles/extra.css` is missing from sources',
  );
});

test('reports a source that does not exist', () => {
  const contract = CONTRACT.replace(
    '  - src/styles/tokens.css',
    '  - src/styles/tokens.css\n  - src/styles/gone.css',
  );
  assertFails(checkDesignDrift(fixture({ contract })), 'sources names `src/styles/gone.css`');
});

test('does not expect the entry stylesheet in the source list', () => {
  const extra = { 'src/styles.css': '@import "./styles/tokens.css";\n' };
  assert.deepEqual(checkDesignDrift(fixture({ extra })), []);
});
