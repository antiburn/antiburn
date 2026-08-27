import assert from 'node:assert/strict';
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';

import { buildAgentCatalog, compareVersions, parseArgs } from './build-tool-catalog.mjs';

/** Writes one antiburn/systemprompts version directory under `agentDir`. */
function writeVersion(agentDir, version, yaml) {
  const dir = join(agentDir, version);
  mkdirSync(dir, { recursive: true });
  writeFileSync(join(dir, 'metadata.yml'), yaml);
}

const CLAUDE_BASH = `
system_prompts:
- model: "claude-sonnet-4-5-20250929"
  tools:
  - canonical_name: "bash"
    observed_raw_aliases:
    - "Bash"
    definition_bytes: 100
    definition_token_count: 300
`;

test('groups a tool by canonical_name and keeps its model token counts', () => {
  const root = mkdtempSync(join(tmpdir(), 'tool-catalog-'));
  try {
    writeVersion(root, '1.0.0', CLAUDE_BASH);
    const catalog = buildAgentCatalog(root, null);
    assert.deepEqual(catalog.versions, { '1.0.0': 0 });
    assert.deepEqual(catalog.surfaces, [
      {
        tools: [
          {
            name: 'bash',
            aliases: ['Bash'],
            tokens: { 'claude-sonnet-4-5-20250929': 300 },
          },
        ],
      },
    ]);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('unions aliases and keeps one token entry per model that measured the tool', () => {
  const root = mkdtempSync(join(tmpdir(), 'tool-catalog-'));
  try {
    writeVersion(
      root,
      '1.0.0',
      `
system_prompts:
- model: "model-a"
  tools:
  - canonical_name: "edit"
    observed_raw_aliases:
    - "Edit"
    definition_bytes: 10
    definition_token_count: 40
- model: "model-b"
  tools:
  - canonical_name: "edit"
    observed_raw_aliases:
    - "StrReplace"
    definition_bytes: 12
    definition_token_count: 55
`,
    );
    const catalog = buildAgentCatalog(root, null);
    assert.deepEqual(catalog.surfaces, [
      {
        tools: [
          {
            name: 'edit',
            aliases: ['Edit', 'StrReplace'],
            tokens: { 'model-a': 40, 'model-b': 55 },
          },
        ],
      },
    ]);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('skips a system_prompts entry with no measured tools, including "unrecorded"', () => {
  const root = mkdtempSync(join(tmpdir(), 'tool-catalog-'));
  try {
    writeVersion(
      root,
      '1.0.0',
      `
system_prompts:
- model: "unrecorded"
  token_count: null
- model: "model-a"
  token_count: null
- model: "model-b"
  tools:
  - canonical_name: "bash"
    observed_raw_aliases:
    - "Bash"
    definition_bytes: 10
    definition_token_count: 20
`,
    );
    const catalog = buildAgentCatalog(root, null);
    assert.deepEqual(catalog.versions, { '1.0.0': 0 });
    assert.deepEqual(catalog.surfaces[0].tools, [
      { name: 'bash', aliases: ['Bash'], tokens: { 'model-b': 20 } },
    ]);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('omits a version whose every system_prompts entry has no measured tools', () => {
  const root = mkdtempSync(join(tmpdir(), 'tool-catalog-'));
  try {
    writeVersion(
      root,
      '1.0.0',
      `
system_prompts:
- model: "unrecorded"
  token_count: null
`,
    );
    const catalog = buildAgentCatalog(root, null);
    assert.deepEqual(catalog.versions, {});
    assert.deepEqual(catalog.surfaces, []);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('keeps a tool with no definition_token_count in the surface, with no tokens entry', () => {
  const root = mkdtempSync(join(tmpdir(), 'tool-catalog-'));
  try {
    writeVersion(
      root,
      '1.0.0',
      `
system_prompts:
- model: "model-a"
  tools:
  - canonical_name: "web_search"
    observed_raw_aliases:
    - "web_search"
    definition_bytes: 48
    definition_token_count: null
`,
    );
    const catalog = buildAgentCatalog(root, null);
    assert.deepEqual(catalog.surfaces[0].tools, [
      { name: 'web_search', aliases: ['web_search'], tokens: {} },
    ]);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('gives two versions with an identical surface the same surface index', () => {
  const root = mkdtempSync(join(tmpdir(), 'tool-catalog-'));
  try {
    writeVersion(root, '1.0.0', CLAUDE_BASH);
    writeVersion(root, '1.0.1', CLAUDE_BASH);
    const catalog = buildAgentCatalog(root, null);
    assert.deepEqual(catalog.versions, { '1.0.0': 0, '1.0.1': 0 });
    assert.equal(catalog.surfaces.length, 1);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('walks versions oldest to newest, so a changed surface gets index 1', () => {
  const root = mkdtempSync(join(tmpdir(), 'tool-catalog-'));
  try {
    // Written newest-first on disk; the catalogue must still walk 1.0.0
    // before 1.0.10, so index 0 is bash-only and index 1 adds edit.
    writeVersion(
      root,
      '1.0.10',
      `
system_prompts:
- model: "model-a"
  tools:
  - canonical_name: "bash"
    observed_raw_aliases:
    - "Bash"
    definition_bytes: 10
    definition_token_count: 20
  - canonical_name: "edit"
    observed_raw_aliases:
    - "Edit"
    definition_bytes: 10
    definition_token_count: 20
`,
    );
    writeVersion(root, '1.0.0', CLAUDE_BASH.replace('claude-sonnet-4-5-20250929', 'model-a'));
    const catalog = buildAgentCatalog(root, null);
    assert.deepEqual(catalog.versions, { '1.0.0': 0, '1.0.10': 1 });
    assert.equal(catalog.surfaces[0].tools.length, 1);
    assert.equal(catalog.surfaces[1].tools.length, 2);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('an allowlist restricts which version directories are read', () => {
  const root = mkdtempSync(join(tmpdir(), 'tool-catalog-'));
  try {
    writeVersion(root, '1.0.0', CLAUDE_BASH);
    writeVersion(root, '1.0.1', CLAUDE_BASH);
    const catalog = buildAgentCatalog(root, new Set(['1.0.1']));
    assert.deepEqual(catalog.versions, { '1.0.1': 0 });
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('compareVersions orders by number, not by digit text', () => {
  assert.ok(compareVersions('1.0.2', '1.0.10') < 0);
  assert.ok(compareVersions('1.0.10', '1.0.2') > 0);
  assert.equal(compareVersions('1.0.0', '1.0.0'), 0);
});

test('parseArgs ignores the literal -- that pnpm run passes through', () => {
  assert.deepEqual(parseArgs(['--', 'checkout', '--out', 'x.json']), {
    checkout: 'checkout',
    out: 'x.json',
    versions: undefined,
  });
});
