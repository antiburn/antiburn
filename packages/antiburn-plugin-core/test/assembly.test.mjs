import assert from 'node:assert/strict'
import { test } from 'node:test'
import { mkdtemp, realpath, readFile, writeFile, rm, mkdir, symlink } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { assemble, verify } from '../../../scripts/assemble-antiburn-shell.mjs'

test('assembly is reproducible, self-contained, licensed, and refuses altered ownership', async () => {
  const temp = await realpath(await mkdtemp(join(tmpdir(), 'antiburn-assembly-test-')))
  try {
    const output = join(temp, 'bundles')
    await assemble(output)
    const first = await readFile(join(output, 'SHA256SUMS'), 'utf8')
    assert.match(first, /claude\/hooks\/register.ts/)
    assert.match(first, /pi\/extensions\/antiburn.mjs/)
    assert.match(first, /opencode\/runtime\/session-checks\.mjs/)
    assert.match(first, /codex\/hooks\/session-check-hook.mjs/)
    assert.match(first, /cursor\/hooks\/session-check-hook.mjs/)
    assert.match(first, /claude-session-checks\/hooks\/session-check-hook.mjs/)
    assert.match(
      first,
      /catalogs\/claude-session-checks\/plugins\/antiburn-session-checks\/lib\/session-checks\.mjs/,
    )
    assert.match(
      first,
      /catalogs\/claude-session-checks\/plugins\/antiburn-session-checks\/lib\/session-checks-validation\.mjs/,
    )
    assert.match(first, /codex\/hooks\/session-check-status.mjs/)
    assert.match(first, /cursor\/hooks\/session-check-status.mjs/)
    assert.doesNotMatch(first, /claude\/lib\/(?:discovery|probe)\.mjs|\.test\./)
    await assemble(output)
    assert.equal(await readFile(join(output, 'SHA256SUMS'), 'utf8'), first)
    await verify(output)
    for (const file of [
      'codex/skills/antiburn/SKILL.md',
      'cursor/skills/antiburn/SKILL.md',
      'opencode/commands/antiburn.md',
    ]) {
      const command = await readFile(join(output, file), 'utf8')
      assert.match(
        command,
        /On the first help invocation only, display the verified availability result/,
      )
      assert.match(command, /Do not repeat the check or notice for subsequent help/)
    }
    const cursorSkill = await readFile(join(output, 'cursor/skills/antiburn/SKILL.md'), 'utf8')
    assert.match(cursorSkill, /name: antiburn\ndisable-model-invocation: true/)
    assert.match(cursorSkill, /Usage: \/antiburn /)
    assert.match(cursorSkill, /fenced text code block/)
    assert.match(cursorSkill, /🔥 `antiburn`/)
    assert.match(cursorSkill, /Keep the heading outside the block/)
    assert.doesNotMatch(cursorSkill, /♨|unless supplied by the host/)
    const codexHooks = JSON.parse(
      await readFile(join(output, 'codex/hooks/hooks.json'), 'utf8'),
    )
    assert.equal(codexHooks.hooks.UserPromptSubmit[0].hooks[0].type, 'command')
    assert.match(codexHooks.hooks.UserPromptSubmit[0].hooks[0].command, /PLUGIN_ROOT/)
    const codexManifest = JSON.parse(
      await readFile(join(output, 'codex/.codex-plugin/plugin.json'), 'utf8'),
    )
    assert.equal(codexManifest.hooks, './hooks/hooks.json')
    const claudeChecksManifest = JSON.parse(
      await readFile(join(output, 'claude-session-checks/.claude-plugin/plugin.json'), 'utf8'),
    )
    assert.equal(claudeChecksManifest.name, 'antiburn-session-checks')
    const claudeChecksHooks = JSON.parse(
      await readFile(join(output, 'claude-session-checks/hooks/hooks.json'), 'utf8'),
    )
    assert.match(
      claudeChecksHooks.hooks.UserPromptSubmit[0].hooks[0].command,
      /CLAUDE_PLUGIN_ROOT/,
    )
    assert.match(claudeChecksHooks.hooks.PostToolUse[0].hooks[0].command, /CLAUDE_PLUGIN_ROOT/)
    const claudeChecksMarketplace = JSON.parse(
      await readFile(
        join(output, 'catalogs/claude-session-checks/.claude-plugin/marketplace.json'),
        'utf8',
      ),
    )
    assert.equal(claudeChecksMarketplace.plugins[0].name, 'antiburn-session-checks')
    const claudeChecksCatalogPlugin =
      'catalogs/claude-session-checks/plugins/antiburn-session-checks'
    assert.match(
      await readFile(
        join(output, claudeChecksCatalogPlugin, 'hooks/session-check-hook.mjs'),
        'utf8',
      ),
      /from '\.\.\/lib\/session-checks\.mjs'/,
    )
    assert.match(
      await readFile(join(output, claudeChecksCatalogPlugin, 'lib/native-hook.mjs'), 'utf8'),
      /acknowledgeSessionChecks/,
    )
    assert.match(
      await readFile(join(output, claudeChecksCatalogPlugin, 'lib/session-checks.mjs'), 'utf8'),
      /session-checks-validation\.mjs/,
    )
    const publicClaudeEntry = await readFile(join(output, 'claude/hooks/register.ts'), 'utf8')
    assert.doesNotMatch(publicClaudeEntry, /session-check/)
    const cursorHooks = JSON.parse(
      await readFile(join(output, 'cursor/hooks/hooks.json'), 'utf8'),
    )
    assert.equal(
      cursorHooks.hooks.postToolUse[0].command,
      'node ./hooks/session-check-hook.mjs',
    )
    for (const host of ['codex', 'cursor']) {
      const hook = await readFile(join(output, `${host}/hooks/session-check-hook.mjs`), 'utf8')
      assert.doesNotMatch(hook, /packages\/antiburn-plugin-core/)
      assert.match(hook, /session-checks\.mjs/)
      const status = await readFile(
        join(output, `${host}/hooks/session-check-status.mjs`),
        'utf8',
      )
      assert.doesNotMatch(status, /packages\/antiburn-plugin-core/)
      assert.match(status, /consumer: 'status'/)
    }
    for (const host of ['pi', 'opencode', 'codex', 'cursor'])
      assert.match(
        await readFile(join(output, `${host}/lib/session-checks.mjs`), 'utf8'),
        /session-checks-validation\.mjs/,
      )
    const opencodeEntry = await readFile(join(output, 'opencode/runtime/antiburn.mjs'), 'utf8')
    assert.match(opencodeEntry, /from '\.\/session-checks\.mjs'/)
    for (const host of ['claude', 'codex', 'cursor', 'opencode', 'pi'])
      assert.match(await readFile(join(output, host, 'LICENSE'), 'utf8'), /MIT License/)
    const claudeEntry = await readFile(join(output, 'claude/hooks/register.ts'), 'utf8')
    assert.doesNotMatch(claudeEntry, /packages\/|\/Users\//)
    assert.match(claudeEntry, /\.\/quota-controller/)
    const releaseEntry = await readFile(join(output, 'claude/hooks/register.ts'), 'utf8')
    const catalogEntry = await readFile(
      join(output, 'catalogs/claude/plugins/antiburn/hooks/register.ts'),
      'utf8',
    )
    assert.equal(catalogEntry, releaseEntry)
    assert.match(first, /catalogs\/claude\/plugins\/antiburn\/lib\/product\.mjs/)
    const catalog = JSON.parse(
      await readFile(join(output, 'catalogs/codex/.agents/plugins/marketplace.json'), 'utf8'),
    )
    assert.equal(catalog.plugins[0].source.path, './plugins/antiburn')
    assert.ok(
      await readFile(
        join(output, 'catalogs/codex', catalog.plugins[0].source.path, 'plugin.json'),
      ),
    )
    await writeFile(join(output, 'codex/skills/antiburn/SKILL.md'), 'user change')
    await assert.rejects(verify(output), /hash/)
    await assert.rejects(assemble(output), /hash/)
  } finally {
    await rm(temp, { recursive: true, force: true })
  }
})

test('assembly refuses unowned output and symlink destinations', async () => {
  const temp = await realpath(await mkdtemp(join(tmpdir(), 'antiburn-assembly-test-')))
  try {
    const existing = join(temp, 'existing')
    await mkdir(existing)
    await writeFile(join(existing, 'mine'), 'keep')
    await assert.rejects(assemble(existing))
    const link = join(temp, 'link')
    await symlink(existing, link)
    await assert.rejects(assemble(link), /symlink/)
    assert.equal(await readFile(join(existing, 'mine'), 'utf8'), 'keep')
  } finally {
    await rm(temp, { recursive: true, force: true })
  }
})
