#!/usr/bin/env node
// Check shared design invariants directly against CSS and native code.
// Exact token values belong to CSS; design.md explains how to use them.

import { existsSync, readFileSync, readdirSync, statSync } from "node:fs";
import { dirname, relative, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";

const REPO_ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const APP_ROOT = resolve(REPO_ROOT, "apps/desktop");
const WINDOW_CORNER_SOURCES = [
  { path: "src-tauri/src/popover.rs", constant: "CORNER_RADIUS" },
  {
    path: "src-tauri/crates/nudge/src/window.rs",
    constant: "NUDGE_CORNER_RADIUS",
  },
];
const norm = (s) => s.replace(/\s+/g, " ").trim();
const normColor = (s) =>
  norm(s).replace(/\d*\.?\d+/g, (n) => String(parseFloat(n)));
const stripComments = (css) => css.replace(/\/\*[\s\S]*?\*\//g, " ");

export function fileSystemIo(root = APP_ROOT) {
  return {
    read: (rel) => readFileSync(resolve(root, rel), "utf8"),
    exists: (rel) => existsSync(resolve(root, rel)),
    listStylesheets: () => listCss(resolve(root, "src"), root),
  };
}

function listCss(dir, root, found = []) {
  for (const entry of readdirSync(dir)) {
    const full = resolve(dir, entry);
    const stats = statSync(full, { throwIfNoEntry: false });
    if (!stats) continue;
    if (stats.isDirectory()) listCss(full, root, found);
    else if (entry.endsWith(".css"))
      found.push(relative(root, full).split(sep).join("/"));
  }
  return found;
}

function themeMap(css) {
  const out = {};
  for (const m of css.matchAll(/--color-([\w-]+):\s*var\(--color-([\w-]+)\)/g))
    out[m[1]] = m[2];
  return out;
}

// Read static fallbacks outside media and live-system overrides.
function themeVars(css, theme) {
  const out = {};
  for (const block of topLevelRuleBodies(css, `:root[data-theme="${theme}"]`)) {
    for (const v of block.matchAll(/--color-([\w-]+):\s*([^;]+);/g)) {
      // A live system token has no static value to compare.
      if (v[2].trim().startsWith("-apple-system")) continue;
      out[v[1]] = v[2].trim();
    }
  }
  return out;
}

// Reduced transparency is a separate axis from the default theme palette.
function systemVars(css, theme) {
  const out = {};
  const add = (blocks) => {
    for (const block of blocks) {
      for (const v of block.matchAll(/--color-([\w-]+):\s*([^;]+);/g)) {
        if (v[2].trim().startsWith("-apple-system")) continue;
        out[v[1]] = v[2].trim();
      }
    }
  };
  add(bareRootBodies(css));
  for (const body of preferenceBlocks(css, theme)) add(bareRootBodies(body));
  return out;
}

function bareRootBodies(css) {
  return topLevelRuleBodies(css, ":root");
}

/** Bodies of the top-level `prefers-color-scheme` blocks for one theme. */
function preferenceBlocks(css, theme) {
  const bodies = [];
  const pattern = new RegExp(
    `@media[^{]*prefers-color-scheme:\\s*${theme}[^{]*\\{`,
    "g",
  );
  for (const m of css.matchAll(pattern)) {
    // The reduced-transparency overrides answer a different setting.
    if (m[0].includes("reduced-transparency")) continue;
    const open = m.index + m[0].length - 1;
    bodies.push(css.slice(open + 1, matchingBrace(css, open)));
  }
  return bodies;
}

// Match a complete selector, not a descendant or a state-specific selector.
function topLevelRuleBodies(css, selector) {
  const bodies = [];
  let cursor = 0;
  while (cursor < css.length) {
    const open = css.indexOf("{", cursor);
    if (open === -1) break;
    const close = matchingBrace(css, open);
    const header = css.slice(cursor, open).split(";").at(-1).trim();
    if (header.split(",").some((part) => part.trim() === selector)) {
      bodies.push(css.slice(open + 1, close));
    }
    cursor = close + 1;
  }
  return bodies;
}

function matchingBrace(css, open) {
  let depth = 1;
  for (let index = open + 1; index < css.length; index += 1) {
    const ch = css[index];
    if (ch === "{") depth += 1;
    else if (ch === "}") {
      depth -= 1;
      if (depth === 0) return index;
    }
  }
  throw new Error("Unclosed CSS block while the theme palette was read");
}

export function checkDesignDrift(io = fileSystemIo()) {
  const failures = [];
  const css = stripComments(
    io
      .listStylesheets()
      .sort()
      .map((file) => io.read(file))
      .join("\n"),
  );
  const map = themeMap(css);
  if (!Object.keys(map).length)
    failures.push("No semantic color aliases found");

  for (const theme of ["light", "dark"]) {
    const explicit = themeVars(css, theme);
    const system = systemVars(css, theme);
    for (const [name, ref] of Object.entries(map)) {
      const forced = explicit[ref];
      const followed = system[ref];
      if (forced === undefined) {
        failures.push(
          `color \`${name}\`: --color-${ref} is not set in the ${theme} palette`,
        );
      }
      if (followed === undefined) {
        failures.push(
          `color \`${name}\`: --color-${ref} is unset in the ${theme} system palette`,
        );
      }
      if (
        forced !== undefined &&
        followed !== undefined &&
        normColor(forced) !== normColor(followed)
      ) {
        failures.push(
          `color \`${name}\` ${theme} = ${forced}, but the system palette sets ${followed}`,
        );
      }
    }
  }

  // Type roles inherit the shared line height so mixed roles keep one rhythm.
  for (const selector of ["html", "body"]) {
    const rules = topLevelRuleBodies(css, selector);
    if (!rules.some((rule) => /line-height:\s*[\d.]+\s*;/.test(rule))) {
      failures.push(`${selector} has no shared unitless line-height`);
    }
  }
  for (const match of css.matchAll(/([^{}]+)\{([^{}]*)\}/g)) {
    if (!/line-height\s*:/.test(match[2])) continue;
    for (const role of new Set(
      [...match[1].matchAll(/\.type-([\w-]+)/g)].map((m) => m[1]),
    )) {
      failures.push(
        `typography \`${role}\`: .type-${role} sets its own line-height`,
      );
    }
  }

  // Native window materials cannot read CSS custom properties.
  const radius = /--radius-popover:\s*([\d.]+)px\s*;/.exec(css)?.[1];
  if (radius === undefined)
    failures.push("--radius-popover is missing or is not a pixel value");
  for (const source of WINDOW_CORNER_SOURCES) {
    if (!io.exists(source.path)) {
      failures.push(`${source.path} not found`);
      continue;
    }
    const pattern = new RegExp(
      `const\\s+${source.constant}:\\s*f64\\s*=\\s*([\\d.]+)\\s*;`,
    );
    const rust = pattern.exec(
      stripComments(io.read(source.path)).replace(/\/\/[^\n]*/g, ""),
    );
    if (!rust) {
      failures.push(`${source.path} declares no ${source.constant}`);
    } else if (
      radius !== undefined &&
      parseFloat(rust[1]) !== parseFloat(radius)
    ) {
      failures.push(
        `--radius-popover ${radius}px != ${source.constant} ${rust[1]} in ${source.path}`,
      );
    }
  }
  return failures;
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const failures = checkDesignDrift();
  if (failures.length) {
    console.error(`Design invariant failures (${failures.length}):\n`);
    for (const failure of failures) console.error(`  - ${failure}`);
    process.exit(1);
  }
  console.log("Design invariants pass");
}
