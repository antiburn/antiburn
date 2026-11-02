import { spawnSync } from 'node:child_process'
import { cp, mkdir, mkdtemp, readFile, realpath, writeFile, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { basename, delimiter, dirname, isAbsolute, join, resolve } from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { defaultOutput, verify } from './assemble-antiburn-core-plugin.mjs'

const hosts = ['claude', 'codex', 'cursor', 'opencode', 'pi']
const owner = 'antiburn-core-plugin-visual-audit-v1'
const json = (value) => JSON.stringify(value, null, 2) + '\n'

function run(binary, args, options) {
  const result = spawnSync(binary, args, { ...options, shell: false })
  if (result.error) throw result.error
  if (result.signal || result.status !== 0)
    throw new Error(`${binary} failed (${result.signal || result.status})`)
}

export async function prepare(artifacts = defaultOutput) {
  artifacts = resolve(artifacts)
  const manifest = await verify(artifacts)
  const directory = await realpath(await mkdtemp(join(tmpdir(), 'antiburn-visual-')))
  try {
    const catalog = `antiburn-audit-${basename(directory).toLowerCase()}`
    await cp(artifacts, join(directory, 'artifacts'), { recursive: true })
    for (const host of hosts) {
      await mkdir(join(directory, host, 'project'), { recursive: true })
      await mkdir(join(directory, host, 'profile'), { recursive: true })
    }
    const cursorCatalog = join(directory, 'artifacts/catalogs/cursor')
    const marketplacePath = join(cursorCatalog, '.cursor-plugin/marketplace.json')
    const marketplace = JSON.parse(await readFile(marketplacePath, 'utf8'))
    marketplace.name = catalog
    await writeFile(marketplacePath, json(marketplace))
    run('git', ['init', '-q', '-b', 'main'], { cwd: cursorCatalog, stdio: 'pipe' })
    run('git', ['add', '.'], { cwd: cursorCatalog, stdio: 'pipe' })
    run(
      'git',
      [
        '-c',
        'user.name=Antiburn Audit Fixture',
        '-c',
        'user.email=fixture@example.invalid',
        '-c',
        'commit.gpgsign=false',
        'commit',
        '-q',
        '-s',
        '-m',
        'test: disposable visual audit catalog',
      ],
      { cwd: cursorCatalog, stdio: 'pipe' },
    )
    const opencodeProject = join(directory, 'opencode/project')
    await mkdir(join(opencodeProject, '.opencode/commands'), { recursive: true })
    await cp(
      join(directory, 'artifacts/opencode/commands/antiburn.md'),
      join(opencodeProject, '.opencode/commands/antiburn.md'),
    )
    await writeFile(
      join(opencodeProject, 'opencode.json'),
      json({
        plugin: [
          pathToFileURL(join(directory, 'artifacts/opencode/runtime/antiburn.mjs')).href,
        ],
      }),
    )
    await mkdir(join(directory, 'cursor/profile/plugins/local'), { recursive: true })
    await cp(
      join(directory, 'artifacts/cursor'),
      join(directory, 'cursor/profile/plugins/local/antiburn'),
      { recursive: true },
    )
    const record = {
      owner,
      directory,
      createdAt: new Date().toISOString(),
      artifactVersion: manifest.version,
      cursorCatalog: catalog,
      sourceHashes: await readFile(join(artifacts, 'SHA256SUMS'), 'utf8'),
      hosts: Object.fromEntries(hosts.map((host) => [host, { visual: 'pending' }])),
    }
    await writeFile(join(directory, 'audit.json'), json(record))
    await writeFile(
      join(directory, 'CHECKLIST.md'),
      `# Antiburn visual audit\n\nAll visual results start pending. Launch success does not certify installation or rendering.\n\nFor each vendor, record its build and capture installation identity, first use, status, ASCII status, help after reload/restart, and removal. Include light/dark and narrow width where supported.\n\nFor Claude, verify the unified antiburn plugin, its commands, quota controls, and optional usage UI. Confirm the candidate contains no shell CLI resolver. Use /antiburn in Claude, Pi, and OpenCode, or the host-assigned collision name. Use /antiburn-plugin for Cursor. In Codex select the plugin help/availability skill, not the CLI findings skill.\n\nFor the remaining local shell candidates, check CLI Found/Not found/Check unavailable and App connection: Not checked. Check glyphs, row breaks, clipping, literal Markdown and duplicate startup notices. The shell must not launch the antiburn executable.\n\n## Cursor\n\nThe launcher provides a separate local profile and plugin candidate. Local loading is not certified. If discovery fails, manually import this Git catalog through Customize: ${cursorCatalog}\n\nAn account-based import may affect other windows or devices. The script does not perform it. Remove only the audit catalog/plugin when finished.\n\n## Codex desktop\n\nDesktop installation and rendering are separate manual gates. Do not install into an active release chat. CLI results do not certify Desktop.\n\n## Isolation and cleanup\n\nProfiles and projects are temporary; this is not an OS sandbox. Vendor environment variables, system credentials and account sync can still apply. No credentials are copied. Sign in manually if required. No model prompts or screenshots are submitted automatically.\n\nExit all audit hosts before removing this directory. No global uninstall or automatic recursive cleanup is supplied.\n`,
    )
    return directory
  } catch (error) {
    await rm(directory, { recursive: true, force: true })
    throw error
  }
}

export async function launch(directory, host, binary) {
  if (!hosts.includes(host)) throw new Error(`Unknown vendor: ${host}`)
  directory = await realpath(directory)
  const record = JSON.parse(await readFile(join(directory, 'audit.json'), 'utf8'))
  if (record.owner !== owner || record.directory !== directory)
    throw new Error('Unowned audit directory')
  binary ??= record.hosts[host]?.binary || host
  if (typeof binary !== 'string' || !binary) throw new Error('Invalid vendor binary')
  const profile = join(directory, host, 'profile')
  const project = join(directory, host, 'project')
  const artifact = join(directory, 'artifacts', host)
  const env = { ...process.env }
  let args = []
  if (host === 'claude') {
    env.CLAUDE_CONFIG_DIR = profile
    args = ['--plugin-dir', artifact]
  } else if (host === 'codex') {
    env.CODEX_HOME = profile
    run(binary, ['plugin', 'marketplace', 'add', join(directory, 'artifacts/catalogs/codex')], {
      cwd: project,
      env,
      stdio: 'inherit',
    })
    run(binary, ['plugin', 'add', 'antiburn@antiburn-core-plugin-local'], {
      cwd: project,
      env,
      stdio: 'inherit',
    })
  } else if (host === 'pi') {
    env.PI_CODING_AGENT_DIR = profile
    args = ['--extension', join(artifact, 'extensions/antiburn.mjs')]
  } else if (host === 'opencode') {
    env.HOME = profile
    env.XDG_CONFIG_HOME = join(profile, 'config')
    env.XDG_DATA_HOME = join(profile, 'data')
    env.XDG_CACHE_HOME = join(profile, 'cache')
    env.XDG_STATE_HOME = join(profile, 'state')
    env.OPENCODE_CONFIG_DIR = join(profile, 'config/opencode')
    delete env.OPENCODE_CONFIG
    delete env.OPENCODE_CONFIG_CONTENT
    delete env.OPENCODE_DISABLE_PROJECT_CONFIG
  } else {
    args = [
      '--user-data-dir',
      profile,
      '--extensions-dir',
      join(directory, 'cursor/extensions'),
      '--new-window',
      project,
    ]
  }
  if (isAbsolute(binary)) env.PATH = `${dirname(binary)}${delimiter}${env.PATH || ''}`
  const startedAt = new Date().toISOString()
  try {
    run(binary, args, { cwd: project, env, stdio: 'inherit' })
    record.hosts[host].lastLaunch = { startedAt, result: 'exited-zero', visual: 'pending' }
  } catch (error) {
    record.hosts[host].lastLaunch = { startedAt, result: 'failed', visual: 'pending' }
    throw error
  } finally {
    await writeFile(join(directory, `${host}-launch.json`), json(record.hosts[host].lastLaunch))
  }
}

async function main(args) {
  if (args[0] === 'prepare' && args.length <= 2) {
    const directory = await prepare(args[1])
    console.log(`Audit prepared: ${directory}`)
    console.log(`Checklist: ${join(directory, 'CHECKLIST.md')}`)
    console.log(
      `Launch: node ${fileURLToPath(import.meta.url)} launch ${directory} <vendor> [binary]`,
    )
  } else if (args[0] === 'launch' && args.length >= 3 && args.length <= 4) {
    await launch(args[1], args[2], args[3])
  } else {
    throw new Error(
      'Usage: audit-antiburn-core-plugin.mjs prepare [artifacts] | launch <audit-dir> <claude|codex|cursor|opencode|pi> [binary]',
    )
  }
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url))
  main(process.argv.slice(2)).catch((error) => {
    console.error(error.message)
    process.exitCode = 1
  })
