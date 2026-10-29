import { createHash } from 'node:crypto'
import {
  lstat,
  mkdir,
  mkdtemp,
  readFile,
  readdir,
  rename,
  rm,
  writeFile,
} from 'node:fs/promises'
import { dirname, join, resolve, relative } from 'node:path'
import { fileURLToPath } from 'node:url'
import { VERSION, CONTRACT_VERSION } from '../packages/antiburn-plugin-core/core.mjs'
import { instructions } from '../packages/antiburn-plugin-core/content.mjs'
import { releaseFiles } from './assemble-claude-mod.mjs'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')
export const defaultOutput = join(root, '.agent-artifacts/antiburn-core-plugin')
const hosts = ['claude', 'codex', 'cursor', 'opencode', 'pi']
const json = (value) => JSON.stringify(value, null, 2) + '\n'
const hash = (value) => createHash('sha256').update(value).digest('hex')
const metadata = {
  name: 'antiburn',
  version: VERSION,
  description: 'antiburn help and executable availability without launching the app.',
  author: { name: 'antiburn' },
  license: 'MIT',
}

async function inventory(directory, prefix = '') {
  const files = []
  for (const entry of await readdir(join(directory, prefix), {
    withFileTypes: true,
  })) {
    const name = prefix ? `${prefix}/${entry.name}` : entry.name
    if (entry.isSymbolicLink()) throw new Error(`Artifact symlink refused: ${name}`)
    if (entry.isDirectory()) files.push(...(await inventory(directory, name)))
    else if (entry.isFile()) files.push(name)
    else throw new Error(`Unsupported artifact file: ${name}`)
  }
  return files.sort()
}

async function rejectSymlinkParents(destination) {
  let current = resolve(destination)
  for (;;) {
    try {
      if ((await lstat(current)).isSymbolicLink()) throw new Error('Output symlink refused')
    } catch (error) {
      if (/** @type {NodeJS.ErrnoException} */ (error).code !== 'ENOENT') throw error
    }
    const parent = dirname(current)
    if (current === parent) return
    current = parent
  }
}

export async function verify(output = defaultOutput) {
  await rejectSymlinkParents(output)
  const manifest = JSON.parse(await readFile(join(output, 'artifact-manifest.json'), 'utf8'))
  if (manifest.owner !== 'antiburn-core-plugin-assembler-v1' || !Array.isArray(manifest.files))
    throw new Error('Unowned artifact output')
  const actual = (await inventory(output)).filter(
    (name) => !['artifact-manifest.json', 'SHA256SUMS'].includes(name),
  )
  if (JSON.stringify(actual) !== JSON.stringify(manifest.files.map((entry) => entry.path)))
    throw new Error('Artifact inventory mismatch')
  for (const entry of manifest.files) {
    if (hash(await readFile(join(output, entry.path))) !== entry.sha256)
      throw new Error(`Artifact hash mismatch: ${entry.path}`)
  }
  const sums = manifest.files.map((entry) => `${entry.sha256}  ${entry.path}\n`).join('')
  if ((await readFile(join(output, 'SHA256SUMS'), 'utf8')) !== sums)
    throw new Error('Artifact hash inventory mismatch')
  return manifest
}

export async function assemble(output = defaultOutput) {
  output = resolve(output)
  if (output === root || !relative(root, output)) throw new Error('Repository output refused')
  await rejectSymlinkParents(output)
  let exists = false
  try {
    await lstat(output)
    exists = true
  } catch (error) {
    if (/** @type {NodeJS.ErrnoException} */ (error).code !== 'ENOENT') throw error
  }
  if (exists) await verify(output)
  await mkdir(dirname(output), { recursive: true })
  const stage = await mkdtemp(join(dirname(output), '.antiburn-core-plugin-stage-'))
  const files = new Map()
  const add = (name, content) => {
    if (files.has(name)) throw new Error(`Duplicate artifact: ${name}`)
    files.set(name, content)
  }
  const read = (name) => readFile(join(root, name), 'utf8')
  try {
    const license = await read('LICENSE')
    for (const host of hosts.filter((name) => name !== 'claude')) {
      add(`${host}/LICENSE`, license)
      add(
        `${host}/core-plugin.json`,
        json({
          id: 'antiburn',
          version: VERSION,
          contractVersion: CONTRACT_VERSION,
          host: host === 'claude' ? 'claude-code' : host,
          certification: 'unverified',
        }),
      )
      add(
        `${host}/README.md`,
        `# antiburn ${VERSION}\n\nLocal ${host} candidate; runtime certification is unverified. See docs/plugins in the source repository for installation and lifecycle gates.\n\n${host === 'claude' ? 'Claude mods API target: 2.1.301. This candidate is generated from the public usage plugin and does not include the separate development checks mod.' : host === 'pi' ? 'Pi API target: 0.80.3. Register extensions/antiburn.mjs. Native TUI startup and explicit TUI/RPC command use the host notification API. User prompts receive Antiburn context before the model call; print/JSON output is unsupported.' : host === 'opencode' ? 'The commands/antiburn.md asset is the explicit model-mediated help/status command. Runtime API target: @opencode-ai/plugin 1.19.2; runtime/antiburn.mjs adds checks to main-session messages with experimental.chat.messages.transform on loopback hosts.' : host === 'codex' ? 'The packaged UserPromptSubmit hook reads staged Antiburn findings and adds them as context without starting a model turn. Codex CLI catalog installation did not activate bundled hooks on tested builds. Run `node hooks/session-check-config.mjs --host codex` to print the native hooks.json fragment, merge it into Codex hooks.json, and review/trust it through the normal host flow. Run `node hooks/session-check-status.mjs --host codex --session SESSION_ID` for read-only delivery status.' : 'The packaged postToolUse hook adds staged Antiburn findings after a tool result. It does not start a new turn. Cursor plugin discovery alone did not activate bundled hooks on tested builds. Run `node hooks/session-check-config.mjs --host cursor` to print the native hooks.json fragment, merge it into Cursor hooks.json, and review/trust it through the normal host flow. Run `node hooks/session-check-status.mjs --host cursor --session SESSION_ID` for read-only delivery status.'}\n\nThe default brand uses a flame emoji and code delimiters around antiburn. Use status --ascii to omit the emoji; --flame remains a default-style alias. No automatic font detection or orange color certification is claimed.\n`,
      )
    }
    for (const host of ['pi', 'opencode', 'codex', 'cursor']) {
      for (const source of ['core.mjs', 'discovery.mjs', 'probe.mjs'])
        add(`${host}/lib/${source}`, await read(`packages/antiburn-plugin-core/${source}`))
      for (const source of [
        'session-checks.mjs',
        'session-checks-validation.mjs',
        'native-hook.mjs',
      ])
        add(`${host}/lib/${source}`, await read(`packages/antiburn-plugin-core/${source}`))
    }
    const claudeFiles = releaseFiles(root)
    for (const [name, content] of claudeFiles) add(`claude/${name}`, content)
    const adapter = async (source) =>
      (await read(source))
        .replaceAll('../../../packages/antiburn-plugin-core/', '../lib/')
        .replaceAll('../../packages/antiburn-plugin-core/', '../lib/')
    const claudeChecksPlugin = 'claude-session-checks'
    const claudeChecksFiles = new Map([
      [
        '.claude-plugin/plugin.json',
        json({
          name: 'antiburn-session-checks',
          version: VERSION,
          description: 'Local-only delivery of predefined Antiburn session checks.',
          author: { name: 'antiburn' },
          hooks: './hooks/hooks.json',
        }),
      ],
      [
        'hooks/hooks.json',
        json({
          hooks: {
            UserPromptSubmit: [
              {
                hooks: [
                  {
                    type: 'command',
                    command: 'node "$CLAUDE_PLUGIN_ROOT/hooks/session-check-hook.mjs"',
                    timeout: 5,
                  },
                ],
              },
            ],
            PostToolUse: [
              {
                hooks: [
                  {
                    type: 'command',
                    command: 'node "$CLAUDE_PLUGIN_ROOT/hooks/session-check-hook.mjs"',
                    timeout: 5,
                  },
                ],
              },
            ],
          },
        }),
      ],
      [
        'hooks/session-check-hook.mjs',
        await adapter('integrations/antiburn/claude/session-check-hook.mjs'),
      ],
      [
        'lib/session-checks.mjs',
        await read('packages/antiburn-plugin-core/session-checks.mjs'),
      ],
      [
        'lib/session-checks-validation.mjs',
        await read('packages/antiburn-plugin-core/session-checks-validation.mjs'),
      ],
      ['lib/native-hook.mjs', await read('packages/antiburn-plugin-core/native-hook.mjs')],
      ['LICENSE', license],
      [
        'README.md',
        '# Antiburn session checks for Claude Code\n\nLocal-only candidate. Install it from the `claude-session-checks` local marketplace with `/plugin marketplace add <path-to-catalogs/claude-session-checks>` and `/plugin install antiburn-session-checks@antiburn-session-checks-local`. Review its hooks before enabling it. This candidate is separate from the public `antiburn` usage plugin. It reads predefined check state on `UserPromptSubmit` and `PostToolUse`; it does not evaluate checks or start another model turn.\n',
      ],
    ])
    for (const [path, content] of claudeChecksFiles)
      add(`${claudeChecksPlugin}/${path}`, content)
    add(
      'codex/plugin.json',
      json({
        $schema: 'https://agent-plugins.org/schemas/1.0.0/plugin.schema.json',
        ...metadata,
      }),
    )
    add(
      'codex/.codex-plugin/plugin.json',
      json({
        name: 'antiburn',
        version: VERSION,
        description: metadata.description,
        hooks: './hooks/hooks.json',
      }),
    )
    add(
      'codex/hooks/hooks.json',
      json({
        hooks: {
          UserPromptSubmit: [
            {
              hooks: [
                {
                  type: 'command',
                  command: 'node "$PLUGIN_ROOT/hooks/session-check-hook.mjs"',
                  command_windows: 'node "$env:PLUGIN_ROOT\\hooks\\session-check-hook.mjs"',
                  timeout: 5,
                },
              ],
            },
          ],
        },
      }),
    )
    add(
      'codex/hooks/session-check-hook.mjs',
      await adapter('integrations/antiburn/codex/session-check-hook.mjs'),
    )
    add(
      'codex/hooks/session-check-config.mjs',
      await adapter('integrations/antiburn/native-hook-config.mjs'),
    )
    add(
      'codex/hooks/session-check-status.mjs',
      await adapter('integrations/antiburn/status-command.mjs'),
    )
    add('codex/skills/antiburn/SKILL.md', instructions('codex', '$antiburn'))
    add('cursor/.cursor-plugin/plugin.json', json(metadata))
    add(
      'cursor/hooks/hooks.json',
      json({
        version: 1,
        hooks: {
          postToolUse: [{ command: 'node ./hooks/session-check-hook.mjs' }],
        },
      }),
    )
    add(
      'cursor/hooks/session-check-hook.mjs',
      await adapter('integrations/antiburn/cursor/session-check-hook.mjs'),
    )
    add(
      'cursor/hooks/session-check-config.mjs',
      await adapter('integrations/antiburn/native-hook-config.mjs'),
    )
    add(
      'cursor/hooks/session-check-status.mjs',
      await adapter('integrations/antiburn/status-command.mjs'),
    )
    add('cursor/skills/antiburn/SKILL.md', instructions('cursor', '/antiburn'))
    add(
      'opencode/commands/antiburn.md',
      instructions('opencode', '/antiburn') + '\nUser arguments: $ARGUMENTS\n',
    )
    add(
      'opencode/runtime/antiburn.mjs',
      await adapter('integrations/antiburn/opencode/antiburn.mjs'),
    )
    add(
      'opencode/runtime/session-checks.mjs',
      await adapter('integrations/antiburn/opencode/session-checks.mjs'),
    )
    add(
      'opencode/package.json',
      json({
        name: 'antiburn-opencode-local',
        version: VERSION,
        private: true,
        license: 'MIT',
        type: 'module',
        exports: './runtime/antiburn.mjs',
        files: ['runtime', 'lib', 'commands', 'LICENSE', 'README.md', 'core-plugin.json'],
      }),
    )
    add('pi/extensions/antiburn.mjs', await adapter('integrations/antiburn/pi/antiburn.mjs'))
    add(
      'pi/package.json',
      json({
        name: 'antiburn-pi-local',
        version: VERSION,
        private: true,
        license: 'MIT',
        type: 'module',
        keywords: ['pi-package'],
        pi: { extensions: ['./extensions/antiburn.mjs'] },
        files: ['extensions', 'lib', 'LICENSE', 'README.md', 'core-plugin.json'],
      }),
    )
    for (const host of ['claude', 'codex', 'cursor']) {
      for (const [name, content] of [...files])
        if (name.startsWith(`${host}/`))
          add(`catalogs/${host}/plugins/antiburn/${name.slice(host.length + 1)}`, content)
      if (host === 'codex')
        add(
          'catalogs/codex/.agents/plugins/marketplace.json',
          json({
            name: 'antiburn-core-plugin-local',
            interface: { displayName: 'antiburn core plugin local candidates' },
            plugins: [
              {
                name: 'antiburn',
                source: { source: 'local', path: './plugins/antiburn' },
                policy: {
                  installation: 'AVAILABLE',
                  authentication: 'ON_INSTALL',
                },
                category: 'Productivity',
              },
            ],
          }),
        )
      else if (host === 'claude')
        add(
          `catalogs/${host}/.${host}-plugin/marketplace.json`,
          json({
            name: 'antiburn-local',
            description: 'Local candidate for the unified antiburn Claude plugin.',
            owner: { name: 'antiburn' },
            plugins: [
              {
                name: 'antiburn',
                source: './plugins/antiburn',
                version: JSON.parse(claudeFiles.get('.claude-plugin/plugin.json')).version,
                description: JSON.parse(claudeFiles.get('.claude-plugin/plugin.json'))
                  .description,
              },
            ],
          }),
        )
      else
        add(
          `catalogs/${host}/.${host}-plugin/marketplace.json`,
          json({
            name: 'antiburn-core-plugin-local',
            description: 'Local antiburn executable availability shell candidates.',
            owner: { name: 'antiburn' },
            plugins: [
              {
                name: 'antiburn',
                source: './plugins/antiburn',
                version: VERSION,
                description: metadata.description,
              },
            ],
          }),
        )
    }
    for (const [path, content] of claudeChecksFiles)
      add(`catalogs/claude-session-checks/plugins/antiburn-session-checks/${path}`, content)
    add(
      'catalogs/claude-session-checks/.claude-plugin/marketplace.json',
      json({
        name: 'antiburn-session-checks-local',
        description: 'Local development marketplace for Antiburn predefined session checks.',
        owner: { name: 'antiburn' },
        plugins: [
          {
            name: 'antiburn-session-checks',
            source: './plugins/antiburn-session-checks',
            version: VERSION,
            description: 'Local-only delivery of predefined Antiburn session checks.',
          },
        ],
      }),
    )
    const entries = [...files].sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0))
    const manifest = {
      owner: 'antiburn-core-plugin-assembler-v1',
      version: VERSION,
      contractVersion: CONTRACT_VERSION,
      files: entries.map(([path, content]) => ({
        path,
        sha256: hash(content),
      })),
    }
    add('artifact-manifest.json', json(manifest))
    add(
      'SHA256SUMS',
      manifest.files.map((entry) => `${entry.sha256}  ${entry.path}\n`).join(''),
    )
    for (const [name, content] of files) {
      if (content.includes(root) || content.includes('claude-mod-release'))
        throw new Error(`Source path leaked: ${name}`)
      await mkdir(dirname(join(stage, name)), { recursive: true })
      await writeFile(join(stage, name), content)
    }
    await verify(stage)
    if (exists) {
      await verify(output)
      const backup = `${stage}-previous`
      await rename(output, backup)
      try {
        await rename(stage, output)
      } catch (error) {
        await rename(backup, output)
        throw error
      }
      await rm(backup, { recursive: true })
    } else await rename(stage, output)
    return manifest
  } finally {
    await rm(stage, { recursive: true, force: true })
  }
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const args = process.argv.slice(2)
  if (args.length > 1 || (args.length && args[0] !== '--verify'))
    throw new Error('Usage: node scripts/assemble-antiburn-core-plugin.mjs [--verify]')
  const result = args[0] === '--verify' ? await verify() : await assemble()
  console.log(
    `antiburn ${result.version}: ${result.files.length} verified files in ${defaultOutput}`,
  )
}
