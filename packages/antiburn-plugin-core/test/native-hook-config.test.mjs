import assert from 'node:assert/strict'
import { mkdtemp, mkdir, readFile, realpath, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { pathToFileURL } from 'node:url'
import { spawnSync } from 'node:child_process'
import { test } from 'node:test'
import {
  parseSessionCheckConfigArgs,
  quotePosix,
  quotePowerShell,
  renderSessionCheckConfig,
  renderSessionCheckConfigOutput,
} from '../../../integrations/antiburn/native-hook-config.mjs'

test('renders Codex UserPromptSubmit configuration for the packaged hook path', () => {
  const hookPath = "/tmp/antiburn's package/hooks/session-check-hook.mjs"
  assert.deepEqual(renderSessionCheckConfig({ host: 'codex', hookPath }), {
    hooks: {
      UserPromptSubmit: [
        {
          hooks: [
            {
              type: 'command',
              command: `node ${quotePosix(hookPath)}`,
              command_windows: `node ${quotePowerShell(hookPath)}`,
              timeout: 5,
            },
          ],
        },
      ],
    },
  })
})

test('renders Cursor postToolUse configuration with the packaged hook path', () => {
  const hookPath = '/tmp/antiburn package/hooks/session-check-hook.mjs'
  assert.deepEqual(renderSessionCheckConfig({ host: 'cursor', hookPath }), {
    version: 1,
    hooks: {
      postToolUse: [{ command: `node ${quotePosix(hookPath)}` }],
    },
  })
})

test('shell quoting preserves spaces, apostrophes, and metacharacters', () => {
  const posix = "/tmp/an'tiburn package/$hook;value.mjs"
  assert.equal(quotePosix(posix), "'/tmp/an'\"'\"'tiburn package/$hook;value.mjs'")
  assert.equal(
    quotePowerShell("C:\\Users\\O'Brien & Sons\\hook.mjs"),
    "'C:\\Users\\O''Brien & Sons\\hook.mjs'",
  )
})

test('rejects invalid arguments, hosts, paths, and unsupported platforms', () => {
  assert.equal(parseSessionCheckConfigArgs(['--host', 'codex']), 'codex')
  for (const args of [
    [],
    ['--host'],
    ['--host', 'claude'],
    ['--host', 'cursor', 'unexpected'],
    ['--host', 'codex', '--unknown'],
    ['--host', 'codex', '--host', 'cursor'],
  ])
    assert.throws(() => parseSessionCheckConfigArgs(args))
  assert.throws(
    () => renderSessionCheckConfig({ host: 'codex', hookPath: 'relative/hook.mjs' }),
    /absolute/,
  )
  assert.throws(
    () =>
      renderSessionCheckConfig({
        host: 'codex',
        hookPath: '/tmp/hook.mjs',
        platform: 'win32',
      }),
    /unsupported on win32/,
  )
})

test('CLI resolves its adjacent packaged hook without running it', async () => {
  const root = await mkdtemp(join(tmpdir(), "antiburn's config preview "))
  try {
    const hookDirectory = join(root, 'hooks')
    await mkdir(hookDirectory)
    const source = new URL(
      '../../../integrations/antiburn/native-hook-config.mjs',
      import.meta.url,
    )
    const configPath = join(hookDirectory, 'session-check-config.mjs')
    const hookPath = join(hookDirectory, 'session-check-hook.mjs')
    await writeFile(configPath, await readFile(source))
    await writeFile(hookPath, "throw new Error('hook must not run')\n")

    const result = spawnSync(process.execPath, [configPath, '--host', 'codex'], {
      encoding: 'utf8',
    })
    assert.equal(result.status, 0, result.stderr)
    const output = JSON.parse(result.stdout)
    assert.equal(
      output.hooks.UserPromptSubmit[0].hooks[0].command,
      `node ${quotePosix(await realpath(hookPath))}`,
    )
    assert.equal(result.stderr, '')

    const invalid = spawnSync(process.execPath, [configPath, '--host', 'invalid'], {
      encoding: 'utf8',
    })
    assert.equal(invalid.status, 2)
    assert.match(invalid.stderr, /Usage:/)
    assert.equal(invalid.stdout, '')

    const cursorConfig = JSON.parse(
      renderSessionCheckConfigOutput(['--host', 'cursor'], pathToFileURL(configPath).href),
    )
    assert.equal(cursorConfig.hooks.postToolUse[0].command, `node ${quotePosix(hookPath)}`)
  } finally {
    await rm(root, { recursive: true, force: true })
  }
})
