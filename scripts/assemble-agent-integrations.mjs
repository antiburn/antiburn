import { lstat, mkdir, mkdtemp, readFile, rename, rm, writeFile } from 'node:fs/promises'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { hash, rejectSymlinkParents, verifyArtifactFiles } from './artifact-files.mjs'

const root = resolve(import.meta.dirname, '..')
export const defaultOutput = join(
  root,
  'apps/desktop/src-tauri/resources/agent-integrations/payload',
)
const json = (value) => JSON.stringify(value, null, 2) + '\n'

async function verifyExisting(output) {
  const manifest = JSON.parse(await readFile(join(output, 'manifest.json'), 'utf8'))
  if (
    manifest.owner !== 'antiburn-desktop-agent-integrations-v1' ||
    manifest.schema !== 1 ||
    !Array.isArray(manifest.files) ||
    manifest.files.length > 64
  )
    throw new Error('Unowned output refused')
  const files = manifest.files.toSorted((a, b) =>
    a.path < b.path ? -1 : a.path > b.path ? 1 : 0,
  )
  await verifyArtifactFiles(output, files, ['manifest.json'])
}

/** Build the desktop-only runtime payload. Public plugins keep their own builder. */
export async function assembleAgentIntegrations(output = defaultOutput) {
  output = resolve(output)
  if (output === root || output === dirname(root)) throw new Error('Unsafe output')
  await rejectSymlinkParents(output)
  const files = new Map()
  const read = (name) => readFile(join(root, name), 'utf8')
  const add = (name, contents) => files.set(name, contents)
  for (const name of [
    'core.mjs',
    'discovery.mjs',
    'probe.mjs',
    'session-checks.mjs',
    'session-checks-validation.mjs',
    'native-hook.mjs',
    'managed-connection.mjs',
  ]) {
    let contents = await read(`packages/antiburn-plugin-core/${name}`)
    if (name === 'managed-connection.mjs') {
      const marker = 'const MANAGED_BY_DESKTOP = false'
      if (contents.split(marker).length !== 2)
        throw new Error('Expected exactly one desktop managed marker')
      contents = contents.replace(marker, 'const MANAGED_BY_DESKTOP = true')
    }
    for (const vendor of ['claude', 'codex', 'cursor', 'opencode', 'pi'])
      add(`${vendor}/lib/${name}`, contents)
  }
  for (const [vendor, entry] of [
    ['claude', 'session-check-hook.mjs'],
    ['codex', 'session-check-hook.mjs'],
    ['cursor', 'session-check-hook.mjs'],
    ['opencode', 'antiburn.mjs'],
    ['opencode', 'session-checks.mjs'],
    ['pi', 'antiburn.mjs'],
    ['pi', 'session-checks.mjs'],
  ]) {
    const source = await read(`integrations/antiburn/${vendor}/${entry}`)
    const directory = ['claude', 'codex', 'cursor'].includes(vendor)
      ? 'hooks'
      : vendor === 'pi'
        ? 'extensions'
        : 'runtime'
    add(
      `${vendor}/${directory}/${entry}`,
      source.replaceAll('../../../packages/antiburn-plugin-core/', '../lib/'),
    )
  }
  add(
    'pi/package.json',
    json({
      name: 'antiburn-desktop-session-checks',
      version: '0.1.0',
      private: true,
      type: 'module',
      pi: { extensions: ['./extensions/session-checks.mjs'] },
    }),
  )
  add('LICENSE', await read('LICENSE'))
  const entries = [...files].sort(([a], [b]) => a.localeCompare(b))
  const manifest = {
    owner: 'antiburn-desktop-agent-integrations-v1',
    schema: 1,
    files: entries.map(([path, content]) => ({ path, sha256: hash(content) })),
  }
  add('manifest.json', json(manifest))
  let exists = false
  try {
    await lstat(output)
    exists = true
  } catch (error) {
    if (!(error instanceof Error && 'code' in error && error.code === 'ENOENT')) throw error
  }
  if (exists) {
    if (output !== defaultOutput) throw new Error('Existing output refused')
    await verifyExisting(output)
  }
  await mkdir(dirname(output), { recursive: true })
  const stage = await mkdtemp(join(dirname(output), '.agent-integrations-stage-'))
  try {
    for (const [name, content] of files) {
      if (content.includes(root)) throw new Error('Source path leaked')
      await mkdir(dirname(join(stage, name)), { recursive: true })
      await writeFile(join(stage, name), content, { flag: 'wx' })
    }
    await verifyExisting(stage)
    await rejectSymlinkParents(output)
    if (exists) {
      await verifyExisting(output)
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
  } finally {
    await rm(stage, { recursive: true, force: true })
  }
  return manifest
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  if (process.argv.length !== 2)
    throw new Error('Usage: node scripts/assemble-agent-integrations.mjs')
  const manifest = await assembleAgentIntegrations()
  console.log(`Built ${manifest.files.length} desktop integration files`)
}
