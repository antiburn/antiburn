import assert from 'node:assert/strict'
import { test } from 'node:test'
import { mkdtemp, readFile, readdir, rm, writeFile, chmod, realpath } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { pathToFileURL } from 'node:url'
import { assemble } from '../../../scripts/assemble-antiburn-core-plugin.mjs'
import { prepare, launch } from '../../../scripts/audit-antiburn-core-plugin.mjs'

test('audit prepares independent profiles and launches only the selected vendor', async () => {
  const temp = await realpath(await mkdtemp(join(tmpdir(), 'antiburn-audit-test-')))
  let audit
  try {
    const artifacts = join(temp, 'artifacts')
    await assemble(artifacts)
    const previousPath = process.env.PATH
    const beforeFailure = (await readdir(tmpdir()))
      .filter((name) => name.startsWith('antiburn-visual-'))
      .sort()
    try {
      process.env.PATH = ''
      await assert.rejects(prepare(artifacts), /ENOENT/)
    } finally {
      if (previousPath === undefined) delete process.env.PATH
      else process.env.PATH = previousPath
    }
    assert.deepEqual(
      (await readdir(tmpdir())).filter((name) => name.startsWith('antiburn-visual-')).sort(),
      beforeFailure,
    )
    audit = await prepare(artifacts)
    const before = await readFile(join(artifacts, 'SHA256SUMS'), 'utf8')
    const config = JSON.parse(
      await readFile(join(audit, 'opencode/project/opencode.json'), 'utf8'),
    )
    assert.equal(
      config.plugin[0],
      pathToFileURL(join(audit, 'artifacts/opencode/runtime/antiburn.mjs')).href,
    )
    const market = JSON.parse(
      await readFile(
        join(audit, 'artifacts/catalogs/cursor/.cursor-plugin/marketplace.json'),
        'utf8',
      ),
    )
    assert.match(market.name, /^antiburn-audit-/)
    assert.ok((await readdir(join(audit, 'artifacts/catalogs/cursor'))).includes('.git'))
    const binary = join(temp, 'fake-vendor')
    await writeFile(
      binary,
      `#!/usr/bin/env node\nconst fs = require('node:fs'); fs.appendFileSync(${JSON.stringify(join(temp, 'calls.jsonl'))}, JSON.stringify({args:process.argv.slice(2),cwd:process.cwd(),env:Object.fromEntries(['CLAUDE_CONFIG_DIR','CODEX_HOME','PI_CODING_AGENT_DIR','HOME','XDG_DATA_HOME','OPENCODE_CONFIG_DIR'].map(k=>[k,process.env[k]]))})+'\\n');\n`,
    )
    await chmod(binary, 0o700)
    for (const host of ['claude', 'codex', 'cursor', 'opencode', 'pi'])
      await launch(audit, host, binary)
    const calls = (await readFile(join(temp, 'calls.jsonl'), 'utf8'))
      .trim()
      .split('\n')
      .map(JSON.parse)
    assert.equal(calls.length, 7)
    assert.equal(calls[0].env.CLAUDE_CONFIG_DIR, join(audit, 'claude/profile'))
    assert.deepEqual(calls[0].args, ['--plugin-dir', join(audit, 'artifacts/claude')])
    assert.equal(calls[1].env.CODEX_HOME, join(audit, 'codex/profile'))
    assert.equal(calls[1].args[0], 'plugin')
    assert.ok(calls[4].args.includes('--user-data-dir'))
    assert.equal(calls[5].env.HOME, join(audit, 'opencode/profile'))
    assert.equal(calls[6].env.PI_CODING_AGENT_DIR, join(audit, 'pi/profile'))
    assert.equal(await readFile(join(artifacts, 'SHA256SUMS'), 'utf8'), before)
    const record = JSON.parse(await readFile(join(audit, 'audit.json'), 'utf8'))
    assert.ok(Object.values(record.hosts).every((host) => host.visual === 'pending'))
    assert.equal(
      JSON.parse(await readFile(join(audit, 'pi-launch.json'), 'utf8')).result,
      'exited-zero',
    )
    record.hosts.pi.binary = binary
    await writeFile(join(audit, 'audit.json'), JSON.stringify(record))
    await launch(audit, 'pi')
    await assert.rejects(launch(audit, 'unknown', binary), /Unknown vendor/)
    await writeFile(join(audit, 'audit.json'), '{}')
    await assert.rejects(launch(audit, 'pi', binary), /Unowned/)
  } finally {
    if (audit) await rm(audit, { recursive: true, force: true })
    await rm(temp, { recursive: true, force: true })
  }
})
