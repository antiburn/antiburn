#!/usr/bin/env node
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

// Design-contract drift guard (dependency-free; no install required).
//
// `apps/desktop/design.md` is the design contract an agent reads before it
// does UI work. Its YAML front matter is a token reference, and the
// stylesheets it names are the source of truth. This script keeps the two
// equal. Without it the contract is prose that can rot in silence.
//
// The token -> CSS-var mapping comes from the `@theme` blocks at run time, so
// this check has no hand-maintained color table to drift. It also reads the
// stylesheet set from the contract's own `sources:` list, and fails when that
// list and the tree disagree.
//
// The check is direction-agnostic. It shows that a value differs. It does not
// say which side is correct.
//
// Run locally: `node scripts/check-design-drift.mjs`

import { existsSync, readFileSync, readdirSync, statSync } from 'node:fs';
import { dirname, relative, resolve, sep } from 'node:path';
import { fileURLToPath } from 'node:url';

const REPO_ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const APP_ROOT = resolve(REPO_ROOT, 'apps/desktop');

/** The design contract, relative to the desktop app. */
const CONTRACT = 'design.md';

// The entry stylesheet only imports the others and holds the app shell rules.
// It declares no tokens, so the contract does not list it as a source.
const ENTRY_STYLESHEET = 'src/styles.css';

// Each shadow token names one selector that must carry that exact box-shadow.
const SHADOW_SELECTORS = {
  popover: 'ui-menu',
  tooltip: 'ui-tooltip',
  raised: 'ui-switch-thumb',
};

// CSS generic font families share the `ui-` prefix with the control classes.
// They belong to a font stack, so the class scan must pass over them.
const GENERIC_FONT_FAMILIES = new Set([
  'ui-monospace',
  'ui-sans-serif',
  'ui-serif',
  'ui-rounded',
]);

const norm = (s) => s.replace(/\s+/g, ' ').trim();

// Compare colors by number, not by text: the contract writes an alpha as
// `0.40` where the CSS writes `0.4`, and both mean the same channel.
const normColor = (s) => norm(s).replace(/\d*\.?\d+/g, (n) => String(parseFloat(n)));

// A comment can hold a selector this file scans for. `tokens.css` names
// `:root[data-theme="dark"]` in prose, next to the rule it explains. Strip
// comments so a sentence about a rule is never read as the rule.
const stripComments = (css) => css.replace(/\/\*[\s\S]*?\*\//g, ' ');

// --- file access ------------------------------------------------------------

/** Read the desktop app from disk. Tests pass an in-memory object instead. */
export function fileSystemIo(root = APP_ROOT) {
  return {
    read: (rel) => readFileSync(resolve(root, rel), 'utf8'),
    exists: (rel) => existsSync(resolve(root, rel)),
    listStylesheets: () => listCss(resolve(root, 'src'), root),
  };
}

function listCss(dir, root, found = []) {
  for (const entry of readdirSync(dir)) {
    const full = resolve(dir, entry);
    const stats = statSync(full, { throwIfNoEntry: false });
    if (!stats) continue;
    if (stats.isDirectory()) listCss(full, root, found);
    else if (entry.endsWith('.css')) found.push(relative(root, full).split(sep).join('/'));
  }
  return found;
}

// --- front-matter parsing ---------------------------------------------------

function frontMatter(text) {
  const m = text.match(/^---\n([\s\S]*?)\n---/);
  if (!m) throw new Error(`${CONTRACT}: no YAML front matter`);
  return m[1];
}

/** Body of a top-level front-matter section. Top-level keys sit at column 0. */
function section(fm, name) {
  const lines = fm.split('\n');
  const start = lines.findIndex((l) => l.startsWith(`${name}:`));
  if (start === -1) return '';
  const out = [];
  for (let i = start + 1; i < lines.length; i += 1) {
    if (/^[A-Za-z]/.test(lines[i])) break;
    out.push(lines[i]);
  }
  return out.join('\n');
}

function docSources(fm) {
  return [...section(fm, 'sources').matchAll(/^\s+-\s+(\S+)/gm)].map((m) => m[1]);
}

/** Colors nest one level: a name, then a `light` and a `dark` value. */
function docColors(fm) {
  const out = {};
  let current = null;
  for (const line of section(fm, 'colors').split('\n')) {
    if (/^\s*#/.test(line)) continue;
    const name = /^ {2}([a-z0-9-]+):\s*(?:#.*)?$/.exec(line);
    if (name) {
      current = name[1];
      out[current] = {};
      continue;
    }
    const value = /^ {4}(light|dark):\s*"([^"]+)"/.exec(line);
    if (value && current) out[current][value[1]] = value[2];
  }
  return out;
}

/**
 * The `# @media <theme>: <value>` note a color may carry.
 *
 * A reader who leaves the app on "System" gets the system-preference branch,
 * not the `[data-theme]` one. Where the two differ, the contract states the
 * system value in this note. Anything the note does not name must agree.
 */
function docColorMedia(fm) {
  const out = {};
  let current = null;
  for (const line of section(fm, 'colors').split('\n')) {
    const name = /^ {2}([a-z0-9-]+):/.exec(line);
    if (name) {
      current = name[1];
      out[current] = {};
      continue;
    }
    const note = /^ {4}(light|dark):.*#\s*@media\s+(light|dark):\s*(.+?)\s*$/.exec(line);
    if (note && current) {
      // The note sits on the value it qualifies; a note for the other theme is
      // a copy-paste slip, so keep both keys and let the check compare them.
      out[current][note[1]] = { theme: note[2], value: note[3] };
    }
  }
  return out;
}

function docTypography(fm) {
  const out = {};
  for (const m of section(fm, 'typography').matchAll(/^\s+([a-z0-9-]+):\s*\{(.+)\}/gm)) {
    const body = m[2];
    const size = /fontSize:\s*([\d.]+)px/.exec(body)?.[1];
    const weight = /fontWeight:\s*(\d+)/.exec(body)?.[1];
    const leading = /lineHeight:\s*"?([\d.]+)/.exec(body)?.[1];
    const tracking = /letterSpacing:\s*"?(-?[\d.]+)/.exec(body)?.[1];
    out[m[1]] = {
      missing: [
        ...(size === undefined ? ['fontSize'] : []),
        ...(weight === undefined ? ['fontWeight'] : []),
        ...(leading === undefined ? ['lineHeight'] : []),
        ...(tracking === undefined ? ['letterSpacing'] : []),
      ],
      size: size === undefined ? null : parseFloat(size),
      weight: weight === undefined ? null : parseInt(weight, 10),
      leading: leading === undefined ? null : parseFloat(leading),
      tracking: tracking === undefined ? null : parseFloat(tracking),
    };
  }
  return out;
}

/** Flat `key: value` pairs. Keys are names, numbers, or CSS custom properties. */
function docPairs(fm, name) {
  const out = {};
  const pattern = /^\s+(--[a-z0-9-]+|[a-z0-9]+):\s*"?([^"\n#]+?)"?\s*(?:#.*)?$/gm;
  for (const m of section(fm, name).matchAll(pattern)) out[m[1]] = m[2].trim();
  return out;
}

// --- CSS parsing ------------------------------------------------------------

/** `--color-surface: var(--color-bg-primary)` becomes `{ surface: 'bg-primary' }`. */
function themeMap(css) {
  const out = {};
  for (const m of css.matchAll(/--color-([\w-]+):\s*var\(--color-([\w-]+)\)/g)) out[m[1]] = m[2];
  return out;
}

/**
 * The palette a theme sets on its explicit `[data-theme]` selector.
 *
 * The contract documents the explicit palettes, not the system-preference
 * branches: a platform without live system tokens gets these values, and they
 * are the only branch stated in full. Nested rules sit below the top level, so
 * the `@media` and `@supports` overrides do not reach this scan.
 */
function themeVars(css, theme) {
  const out = {};
  for (const block of topLevelRuleBodies(css, `:root[data-theme="${theme}"]`)) {
    for (const v of block.matchAll(/--color-([\w-]+):\s*([^;]+);/g)) {
      // A live system token has no static value to compare.
      if (v[2].trim().startsWith('-apple-system')) continue;
      out[v[1]] = v[2].trim();
    }
  }
  return out;
}

/**
 * The palette a reader gets while the app follows the system.
 *
 * The bare `:root` holds the whole palette, and a `prefers-color-scheme`
 * block states only what that theme changes. Both themes have one: light
 * restates the tertiary label the `@supports` block just made live. The
 * reduced-transparency blocks are a separate axis and stay out.
 *
 * A live system token has no static value, so it is skipped here exactly as
 * it is in [`themeVars`]. That is what keeps the four `-apple-system-*` tokens
 * out of the comparison: they are the point of following the system.
 */
function systemVars(css, theme) {
  const out = {};
  const add = (blocks) => {
    for (const block of blocks) {
      for (const v of block.matchAll(/--color-([\w-]+):\s*([^;]+);/g)) {
        if (v[2].trim().startsWith('-apple-system')) continue;
        out[v[1]] = v[2].trim();
      }
    }
  };
  add(bareRootBodies(css));
  for (const body of preferenceBlocks(css, theme)) add(bareRootBodies(body));
  return out;
}

/**
 * Bodies of every top-level rule whose selector list holds a bare `:root`.
 *
 * `:root, :root[data-theme="light"]` counts: the bare half applies while the
 * app follows the system. `:root[data-theme="light"]` alone does not, so the
 * character after the selector has to be one that ends it.
 */
function bareRootBodies(css) {
  const bodies = [];
  let depth = 0;
  for (let index = 0; index < css.length; index += 1) {
    if (depth === 0 && css.startsWith(':root', index) && /^[\s,{]$/.test(css[index + 5] ?? '')) {
      const open = css.indexOf('{', index + 5);
      if (open === -1) break;
      const close = matchingBrace(css, open);
      bodies.push(css.slice(open + 1, close));
      index = close;
      continue;
    }
    const ch = css[index];
    if (ch === '{') depth += 1;
    else if (ch === '}') depth = Math.max(0, depth - 1);
  }
  return bodies;
}

/** Bodies of the top-level `prefers-color-scheme` blocks for one theme. */
function preferenceBlocks(css, theme) {
  const bodies = [];
  const pattern = new RegExp(`@media[^{]*prefers-color-scheme:\\s*${theme}[^{]*\\{`, 'g');
  for (const m of css.matchAll(pattern)) {
    // The reduced-transparency overrides answer a different setting.
    if (m[0].includes('reduced-transparency')) continue;
    const open = m.index + m[0].length - 1;
    bodies.push(css.slice(open + 1, matchingBrace(css, open)));
  }
  return bodies;
}

/** Bodies of every rule that names the selector at the top level of the file. */
function topLevelRuleBodies(css, selector) {
  const bodies = [];
  let depth = 0;
  for (let index = 0; index < css.length; index += 1) {
    if (depth === 0 && css.startsWith(selector, index)) {
      const open = css.indexOf('{', index + selector.length);
      if (open === -1) break;
      const close = matchingBrace(css, open);
      bodies.push(css.slice(open + 1, close));
      index = close;
      continue;
    }
    const ch = css[index];
    if (ch === '{') depth += 1;
    else if (ch === '}') depth = Math.max(0, depth - 1);
  }
  return bodies;
}

function matchingBrace(css, open) {
  let depth = 1;
  for (let index = open + 1; index < css.length; index += 1) {
    const ch = css[index];
    if (ch === '{') depth += 1;
    else if (ch === '}') {
      depth -= 1;
      if (depth === 0) return index;
    }
  }
  throw new Error('Unclosed CSS block while the theme palette was read');
}

function cssTypography(css, name) {
  const body = new RegExp(`\\.type-${name}\\s*\\{([\\s\\S]*?)\\}`).exec(css)?.[1];
  if (!body) return null;
  const size = /font-size:\s*([\d.]+)px/.exec(body)?.[1];
  const weight = /font-weight:\s*(\d+)/.exec(body)?.[1];
  const tracking = /letter-spacing:\s*(-?[\d.]+)/.exec(body)?.[1];
  return {
    missing: [
      ...(size === undefined ? ['font-size'] : []),
      ...(weight === undefined ? ['font-weight'] : []),
      ...(tracking === undefined ? ['letter-spacing'] : []),
    ],
    size: size === undefined ? null : parseFloat(size),
    weight: weight === undefined ? null : parseInt(weight, 10),
    tracking: tracking === undefined ? null : parseFloat(tracking),
  };
}

function cssBoxShadow(css, selector) {
  const body = new RegExp(`\\.${selector}\\s*\\{([\\s\\S]*?)\\}`).exec(css)?.[1];
  if (!body) return { value: null, error: `.${selector} not found in the CSS` };
  const shadow = /box-shadow:\s*([\s\S]*?);/.exec(body)?.[1];
  if (!shadow) return { value: null, error: `.${selector} sets no box-shadow` };
  return { value: norm(shadow), error: null };
}

// --- checks -----------------------------------------------------------------

export function checkDesignDrift(io = fileSystemIo()) {
  const failures = [];
  const fail = (msg) => failures.push(msg);

  const docText = io.read(CONTRACT);
  const fm = frontMatter(docText);
  const sources = docSources(fm);

  // The source list must name every stylesheet the app has. A stylesheet the
  // contract does not know about is a token set nothing checks.
  const listed = new Set(sources);
  const present = io.listStylesheets().filter((p) => p !== ENTRY_STYLESHEET);
  for (const file of present) {
    if (!listed.has(file)) fail(`stylesheet \`${file}\` is missing from sources`);
  }
  for (const file of sources) {
    if (!io.exists(file)) fail(`sources names \`${file}\`, which does not exist`);
  }

  const cssAll = stripComments(
    sources
      .filter((file) => io.exists(file))
      .map((file) => io.read(file))
      .join('\n'),
  );

  // Colors: coverage first, then the value in each theme.
  const map = themeMap(cssAll);
  const colors = docColors(fm);
  for (const name of Object.keys(map)) {
    if (!(name in colors)) fail(`@theme token \`${name}\` is not documented in colors`);
  }
  for (const name of Object.keys(colors)) {
    if (!map[name]) fail(`color \`${name}\` is not registered in any @theme block`);
  }
  for (const theme of ['light', 'dark']) {
    const vars = themeVars(cssAll, theme);
    for (const [name, values] of Object.entries(colors)) {
      const ref = map[name];
      if (!ref) continue;
      const documented = values[theme];
      if (documented === undefined) {
        fail(`color \`${name}\` has no ${theme} value`);
        continue;
      }
      const actual = vars[ref];
      if (actual === undefined) {
        fail(`color \`${name}\`: --color-${ref} is not set in the ${theme} palette`);
        continue;
      }
      if (norm(documented) !== norm(actual)) {
        fail(`color \`${name}\` ${theme} = ${documented}, expected ${actual} (--color-${ref})`);
      }
    }
  }

  // The system-preference palette against the explicit one.
  //
  // The contract states the `[data-theme]` values, which a reader gets only
  // after forcing a theme. On "System" the bare `:root` and the
  // `prefers-color-scheme` block apply instead. The two are one palette
  // written twice, so every token must agree unless the contract states the
  // system value in a `# @media <theme>: <value>` note.
  const media = docColorMedia(fm);
  for (const theme of ['light', 'dark']) {
    const explicit = themeVars(cssAll, theme);
    const system = systemVars(cssAll, theme);
    for (const [name, values] of Object.entries(colors)) {
      const ref = map[name];
      if (!ref || values[theme] === undefined) continue;
      const note = media[name]?.[theme];
      const forced = explicit[ref];
      const followed = system[ref];
      // A missing explicit value is already reported by the check above.
      if (forced === undefined) continue;
      if (followed === undefined) {
        fail(`color \`${name}\`: --color-${ref} is unset in the ${theme} system palette`);
        continue;
      }
      if (normColor(forced) === normColor(followed)) {
        if (note) fail(`color \`${name}\` ${theme}: the @media note repeats the value; drop it`);
        continue;
      }
      if (!note) {
        fail(
          `color \`${name}\` ${theme} = ${forced}, but the system palette sets ` +
            `${followed}; state it as a \`# @media ${theme}: ${followed}\` note`,
        );
        continue;
      }
      if (note.theme !== theme) {
        fail(`color \`${name}\` ${theme}: the note names @media ${note.theme}`);
        continue;
      }
      if (normColor(note.value) !== normColor(followed)) {
        fail(`color \`${name}\` @media ${theme} note = ${note.value}, expected ${followed}`);
      }
    }
  }

  // Typography: the `.type-*` classes set size, weight, and tracking. The
  // line-height comes from the base element rules, so compare it to those.
  // A step that sets its own line-height breaks that one-value assumption, and
  // the contract could no longer state the leading once. Report it instead.
  for (const m of cssAll.matchAll(/\.type-([\w-]+)\s*\{([\s\S]*?)\}/g)) {
    if (/line-height:/.test(m[2])) {
      fail(`typography \`${m[1]}\`: .type-${m[1]} sets its own line-height`);
    }
  }
  const baseLeading = /line-height:\s*([\d.]+)\s*;/.exec(cssAll)?.[1];
  for (const [name, t] of Object.entries(docTypography(fm))) {
    if (t.missing.length) {
      fail(`typography \`${name}\`: front matter is missing ${t.missing.join(', ')}`);
      continue;
    }
    const css = cssTypography(cssAll, name);
    if (!css) {
      fail(`typography \`${name}\`: .type-${name} not found in the CSS`);
      continue;
    }
    if (css.missing.length) {
      fail(`typography \`${name}\`: .type-${name} is missing ${css.missing.join(', ')}`);
      continue;
    }
    if (t.size !== css.size) fail(`type \`${name}\` fontSize ${t.size} != ${css.size}`);
    if (t.weight !== css.weight) fail(`type \`${name}\` fontWeight ${t.weight} != ${css.weight}`);
    if (Math.abs(t.tracking - css.tracking) > 1e-6) {
      fail(`type \`${name}\` letterSpacing ${t.tracking} != ${css.tracking}`);
    }
    if (baseLeading === undefined) {
      fail(`type \`${name}\` lineHeight: no base line-height found in the CSS`);
    } else if (Math.abs(t.leading - parseFloat(baseLeading)) > 1e-6) {
      fail(`type \`${name}\` lineHeight ${t.leading} != the base ${baseLeading}`);
    }
  }

  // Spacing rhythm against the --space-* vars.
  const cssSpace = [...cssAll.matchAll(/--space-[a-z\d]+:\s*(\d+)px/g)]
    .map((m) => Number(m[1]))
    .sort((a, b) => a - b);
  const sp = docPairs(fm, 'spacing');
  const steps = Object.entries(sp)
    .filter(([k]) => /^\d+$/.test(k))
    .map(([, v]) => parseInt(v, 10))
    .sort((a, b) => a - b);
  if (JSON.stringify(steps) !== JSON.stringify(cssSpace)) {
    fail(`spacing ${JSON.stringify(steps)} != --space-* ${JSON.stringify(cssSpace)}`);
  }
  if (parseInt(sp.base, 10) !== cssSpace[0]) {
    fail(`spacing.base ${sp.base} != ${cssSpace[0]}px`);
  }

  // Radii. `full` is a plain CSS value with no var, so it has nothing to match.
  const rounded = docPairs(fm, 'rounded');
  for (const key of ['small', 'control', 'popover']) {
    const css = new RegExp(`--radius-${key}:\\s*(\\d+)px`).exec(cssAll)?.[1];
    if (css === undefined) {
      fail(`rounded.${key}: --radius-${key} not found in the CSS`);
      continue;
    }
    if (parseInt(rounded[key], 10) !== parseInt(css, 10)) {
      fail(`rounded.${key} ${rounded[key]} != --radius-${key} ${css}px`);
    }
  }

  // Custom properties documented under `sizes` and `motion`. A group may also
  // hold prose recipes, which carry no var name and so have nothing to compare.
  for (const group of ['sizes', 'motion']) {
    for (const [key, value] of Object.entries(docPairs(fm, group))) {
      if (!key.startsWith('--')) continue;
      const css = new RegExp(`${key}:\\s*([^;]+);`).exec(cssAll)?.[1];
      if (css === undefined) {
        fail(`${group} ${key}: not found in the CSS`);
        continue;
      }
      if (norm(value) !== norm(css)) fail(`${group} ${key} = ${value}, expected ${norm(css)}`);
    }
  }

  // Shadows against the selectors that carry them. Every documented shadow
  // needs a selector, or it is a value nothing compares.
  const shadow = docPairs(fm, 'shadow');
  for (const key of Object.keys(shadow)) {
    if (!(key in SHADOW_SELECTORS)) fail(`shadow.${key} has no selector to check it against`);
  }
  for (const [key, selector] of Object.entries(SHADOW_SELECTORS)) {
    const css = cssBoxShadow(cssAll, selector);
    if (css.error) {
      fail(`shadow.${key}: ${css.error}`);
      continue;
    }
    if (norm(shadow[key] ?? '') !== css.value) {
      fail(`shadow.${key} does not match the .${selector} box-shadow`);
    }
  }

  // Reference integrity: the prose names real files and real classes.
  for (const m of docText.matchAll(/\bsrc\/[\w./-]+\.(?:ts|tsx|css)\b/g)) {
    if (!io.exists(m[0])) fail(`referenced path does not exist: ${m[0]}`);
  }
  for (const m of docText.matchAll(/\b(?:ui|type)-[a-z][\w-]*/g)) {
    if (GENERIC_FONT_FAMILIES.has(m[0])) continue;
    if (!cssAll.includes(m[0])) fail(`referenced class not found in the CSS: ${m[0]}`);
  }

  // Every {group.key} resolves inside the front matter.
  const groups = ['colors', 'typography', 'spacing', 'rounded', 'shadow', 'fonts'];
  const keys = {};
  for (const g of groups) {
    keys[g] = new Set([...section(fm, g).matchAll(/^\s+([a-z0-9-]+):/gm)].map((m) => m[1]));
  }
  for (const m of fm.matchAll(/\{([a-z]+)\.([a-z0-9-]+)\}/g)) {
    if (!keys[m[1]]?.has(m[2])) fail(`unresolved interpolation {${m[1]}.${m[2]}}`);
  }

  return failures;
}

// --- CLI --------------------------------------------------------------------

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const failures = checkDesignDrift();
  if (failures.length) {
    console.error(`design-contract drift detected (${failures.length}):\n`);
    for (const f of failures) console.error(`  - ${f}`);
    console.error('\nUpdate apps/desktop/design.md to match the CSS, or the CSS to match it.');
    process.exit(1);
  }
  console.log('design contract is in sync with the stylesheets');
}
