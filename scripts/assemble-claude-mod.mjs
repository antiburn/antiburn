#!/usr/bin/env node
import {
  mkdtempSync,
  lstatSync,
  mkdirSync,
  readFileSync,
  readdirSync,
  renameSync,
  rmSync,
  writeFileSync,
} from 'node:fs'
import { execFileSync } from 'node:child_process'
import { createHash } from 'node:crypto'
import { dirname, join, relative, resolve, sep } from 'node:path'
import { fileURLToPath } from 'node:url'

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const SOURCE = 'plugins/antiburn-live'
export const PUBLIC = 'plugins/antiburn'
export const LOCAL = '.agent-artifacts/claude-mod'
const RELEASE_FILES = [
  '.claude-plugin/plugin.json',
  'README.md',
  'CHANGELOG.md',
  'PRIVACY.md',
  'hooks/account-usage.ts',
  'hooks/checking-status.ts',
  'hooks/usage-graphics.ts',
  'hooks/quota-controller.tsx',
  'types/index.d.ts',
  'assets/icon.png',
]
const SHARED_FILES = [
  'hooks/brand.ts',
  'hooks/usage.ts',
  'hooks/usage-presentation.ts',
  'hooks/usage-meter-graphics.ts',
  'hooks/usage-meter-band.tsx',
  'hooks/usage-font.ts',
  'hooks/usage-font-data.ts',
  'hooks/usage-target.tsx',
  'hooks/hooks.json',
  'types/claude-code.d.ts',
  'assets/bitcount/OFL.txt',
  'assets/bitcount/README.md',
]
const json = (value) => Buffer.from(`${JSON.stringify(value, null, 2)}\n`)

function containedFile(root, path) {
  const parts = path.split('/')
  let current = root
  for (const part of parts) {
    if (!part || part === '.' || part === '..') throw new Error(`Unsafe path: ${path}`)
    current = join(current, part)
    if (lstatSync(current).isSymbolicLink()) throw new Error(`Symbolic link: ${path}`)
  }
  if (!lstatSync(current).isFile()) throw new Error(`Not a regular file: ${path}`)
  return readFileSync(current)
}

export function releaseFiles(root = ROOT) {
  const files = new Map()
  for (const path of RELEASE_FILES)
    files.set(path, containedFile(root, `${SOURCE}/release/${path}`))
  for (const path of SHARED_FILES) files.set(path, containedFile(root, `${SOURCE}/${path}`))
  files.set('LICENSE', containedFile(root, 'LICENSE'))
  const manifest = JSON.parse(files.get('.claude-plugin/plugin.json'))
  if (manifest.name !== 'antiburn') throw new Error('Unexpected public plugin identity')
  files.set('hooks/register.ts', containedFile(root, 'integrations/antiburn/claude/register.ts'))
  const commands = containedFile(root, 'integrations/antiburn/claude/commands.ts')
    .toString()
    .replaceAll('../../../packages/antiburn-plugin-core/product.mjs', '../lib/product.mjs')
    .replaceAll(
      '../../../plugins/antiburn-live/release/hooks/quota-controller',
      './quota-controller',
    )
  files.set('hooks/commands.ts', Buffer.from(commands))
  files.set(
    'hooks/quota-controller.tsx',
    Buffer.from(
      files
        .get('hooks/quota-controller.tsx')
        .toString()
        .replaceAll('../../../../packages/antiburn-plugin-core/product.mjs', '../lib/product.mjs')
        .replaceAll('../../../../integrations/antiburn/claude/commands', './commands'),
    ),
  )
  const register = containedFile(root, 'integrations/antiburn/claude/register.ts')
    .toString()
    .replaceAll(
      '../../../plugins/antiburn-live/release/hooks/quota-controller',
      './quota-controller',
    )
  files.set('hooks/register.ts', Buffer.from(register))
  files.set('lib/product.mjs', containedFile(root, 'packages/antiburn-plugin-core/product.mjs'))
  files.set('hooks/hooks.json', json({ modules: ['./quota-controller.tsx'] }))
  assertImports(files)
  return files
}

export function assertImports(files, allowedImports = []) {
  for (const [path, bytes] of files) {
    if (!/\.(?:js|mjs|ts|tsx)$/.test(path)) continue
    const source = bytes.toString()
    if (/\bimport\s*\(\s*(?!['"])/.test(source))
      throw new Error(`Non-literal dynamic import in ${path}`)
    for (const match of source.matchAll(
      /\b(?:import(?:\s+type)?\s+(?:[\w*\s{},]+?\s+from\s+)?|export\s+(?:type\s+)?(?:\*|\{[^}]*\})\s+from\s+|import\s*\(\s*)(['"])([^'"]+)\1/g,
    )) {
      const target = match[2]
      if (target === 'claude-code' || target.startsWith('claude-code/')) continue
      if (allowedImports.includes(target)) continue
      if (!target.startsWith('.')) throw new Error(`Unapproved import in ${path}: ${target}`)
      const normalized = relative('/artifact', resolve('/artifact', dirname(path), target))
      if (normalized.startsWith('..')) throw new Error(`Escaped import in ${path}: ${target}`)
      const extensionless = normalized.replace(/\.(?:c|m)?js$/, '')
      if (
        ![
          normalized,
          `${normalized}.ts`,
          `${normalized}.tsx`,
          `${normalized}.d.ts`,
          `${normalized}/index.ts`,
          `${normalized}/index.tsx`,
          `${normalized}/index.d.ts`,
          `${normalized}.js`,
          `${normalized}.mjs`,
          `${normalized}/index.js`,
          `${normalized}/index.mjs`,
          `${extensionless}.ts`,
          `${extensionless}.tsx`,
          `${extensionless}.d.ts`,
          `${extensionless}.js`,
          `${extensionless}.mjs`,
        ].some((name) => files.has(name))
      ) {
        throw new Error(`Missing or escaped import in ${path}: ${target}`)
      }
    }
  }
}

function listFiles(root, prefix = '') {
  const found = []
  for (const entry of readdirSync(join(root, prefix), {
    withFileTypes: true,
  })) {
    const path = prefix ? `${prefix}/${entry.name}` : entry.name
    if (entry.isSymbolicLink()) throw new Error(`Symbolic link: ${path}`)
    if (entry.isDirectory()) found.push(...listFiles(root, path))
    else if (entry.isFile()) found.push(path)
    else throw new Error(`Not a regular file: ${path}`)
  }
  return found.sort()
}

export function checkTree(destination, expected) {
  if (lstatSync(destination).isSymbolicLink()) throw new Error(`Symbolic link: ${destination}`)
  const actual = listFiles(destination)
  const missing = [...expected.keys()].filter((path) => !actual.includes(path))
  const extra = actual.filter((path) => !expected.has(path))
  const changed = actual.filter(
    (path) =>
      expected.has(path) && !readFileSync(join(destination, path)).equals(expected.get(path)),
  )
  if (missing.length || extra.length || changed.length) {
    throw new Error(`Artifact drift: ${JSON.stringify({ missing, extra, changed })}`)
  }
}

function containedDestination(root, destination) {
  const path = relative(root, destination)
  if (!path || path.startsWith('..')) throw new Error(`Unsafe destination: ${destination}`)
  let current = root
  for (const part of path.split('/')) {
    current = join(current, part)
    if (lstatSync(current, { throwIfNoEntry: false })?.isSymbolicLink())
      throw new Error(`Symbolic link: ${current}`)
  }
}

function treeHashes(destination) {
  return Object.fromEntries(
    listFiles(destination).map((path) => [
      path,
      createHash('sha256')
        .update(readFileSync(join(destination, path)))
        .digest('hex'),
    ]),
  )
}

function ownershipPath(root, destination) {
  const relativePath = relative(root, destination)
  const key = createHash('sha256').update(relativePath).digest('hex')
  return join(root, '.agent-artifacts/assembler-ownership', `${key}.json`)
}

function matchesTrackedTree(root, destination, current) {
  const relativeDestination = relative(root, destination).split(sep).join('/')
  let records
  try {
    records = execFileSync('git', ['ls-files', '--stage', '-z', '--', relativeDestination], {
      cwd: root,
      maxBuffer: 16 * 1024 * 1024,
      stdio: ['ignore', 'pipe', 'ignore'],
    })
  } catch {
    return false
  }
  const prefix = `${relativeDestination}/`
  const tracked = new Map()
  for (const record of records.toString().split('\0').filter(Boolean)) {
    const tab = record.indexOf('\t')
    if (tab < 0) return false
    const [mode, object, stage] = record.slice(0, tab).split(' ')
    const path = record.slice(tab + 1)
    if (stage !== '0' || mode === '120000' || !path.startsWith(prefix)) return false
    tracked.set(path.slice(prefix.length), object)
  }
  const actualPaths = Object.keys(current).sort()
  if (tracked.size === 0 || actualPaths.length !== tracked.size) return false
  for (const [path, object] of tracked) {
    if (!Object.hasOwn(current, path)) return false
    try {
      const expected = execFileSync('git', ['cat-file', 'blob', object], {
        cwd: root,
        maxBuffer: 16 * 1024 * 1024,
        stdio: ['ignore', 'pipe', 'ignore'],
      })
      if (!readFileSync(join(destination, path)).equals(expected)) return false
    } catch {
      return false
    }
  }
  return true
}

function writeTree(root, destination, files) {
  containedDestination(root, destination)
  if (lstatSync(destination, { throwIfNoEntry: false })?.isSymbolicLink())
    throw new Error(`Symbolic link: ${destination}`)
  const expected = Object.fromEntries(
    [...files]
      .sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0))
      .map(([path, bytes]) => [path, createHash('sha256').update(bytes).digest('hex')]),
  )
  const owner = ownershipPath(root, destination)
  const currentExists = Boolean(lstatSync(destination, { throwIfNoEntry: false }))
  if (currentExists) {
    const current = treeHashes(destination)
    if (JSON.stringify(current) === JSON.stringify(expected)) return
    let recorded
    try {
      recorded = JSON.parse(readFileSync(owner, 'utf8'))
    } catch {
      if (!matchesTrackedTree(root, destination, current))
        throw new Error(`Refusing to replace unowned output: ${destination}`)
    }
    if (
      recorded &&
      (recorded.owner !== 'antiburn-claude-assembler-v1' ||
        recorded.files === undefined ||
        JSON.stringify(current) !== JSON.stringify(recorded.files))
    )
      throw new Error(`Refusing to replace modified or unowned output: ${destination}`)
  }

  const parent = dirname(destination)
  mkdirSync(parent, { recursive: true })
  const stage = mkdtempSync(join(parent, `.${destination.split('/').at(-1)}.stage-`))
  const backup = `${stage}-previous`
  const ownerDirectory = dirname(owner)
  mkdirSync(ownerDirectory, { recursive: true })
  const ownerStage = `${owner}.${process.pid}.tmp`
  try {
    for (const [path, bytes] of [...files].sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0))) {
      const target = join(stage, path)
      mkdirSync(dirname(target), { recursive: true })
      writeFileSync(target, bytes)
    }
    if (JSON.stringify(treeHashes(stage)) !== JSON.stringify(expected))
      throw new Error(`Staged output mismatch: ${destination}`)
    writeFileSync(
      ownerStage,
      JSON.stringify({ owner: 'antiburn-claude-assembler-v1', files: expected }, null, 2) + '\n',
    )
    if (currentExists) renameSync(destination, backup)
    try {
      renameSync(stage, destination)
      renameSync(ownerStage, owner)
    } catch (error) {
      rmSync(destination, { recursive: true, force: true })
      if (currentExists) renameSync(backup, destination)
      throw error
    }
    if (currentExists) rmSync(backup, { recursive: true, force: true })
  } finally {
    rmSync(stage, { recursive: true, force: true })
    rmSync(backup, { recursive: true, force: true })
    rmSync(ownerStage, { force: true })
  }
}

export function rewriteDevSource(source) {
  return source
    .replaceAll('antiburn-live', 'antiburn-dev')
    .replaceAll('antiburn-mod-dev', 'antiburn-dev')
}

function devFiles(root, tests = false) {
  const files = new Map()
  const sourceRoot = join(root, SOURCE)
  for (const path of listFiles(sourceRoot)) {
    if (
      path.startsWith('release/') ||
      path.startsWith('.claude-plugin/types/') ||
      (!tests && /\.test\.tsx?$/.test(path)) ||
      path === 'tsconfig.json'
    )
      continue
    const bytes = containedFile(sourceRoot, path)
    const text = bytes.toString()
    const rewrittenText = rewriteDevSource(text)
      .replaceAll('../../../packages/antiburn-plugin-core/product.mjs', '../lib/product.mjs')
      .replaceAll('../../../packages/antiburn-plugin-core/core.mjs', '../lib/core.mjs')
      .replaceAll(
        '../../../packages/antiburn-plugin-core/session-checks-validation.mjs',
        '../lib/session-checks-validation.mjs',
      )
    files.set(
      path,
      /\.(?:ts|tsx|md|json)$/.test(path)
        ? Buffer.from(
            tests
              ? rewrittenText.replaceAll(
                  '../../../integrations/antiburn/claude/register',
                  '../release-reference/hooks/register',
                )
              : rewrittenText,
          )
        : bytes,
    )
  }
  files.set('lib/product.mjs', containedFile(root, 'packages/antiburn-plugin-core/product.mjs'))
  for (const path of [
    'core.mjs',
    'discovery.mjs',
    'probe.mjs',
    'session-checks-validation.mjs',
  ])
    files.set(`lib/${path}`, containedFile(root, `packages/antiburn-plugin-core/${path}`))
  const manifest = JSON.parse(files.get('.claude-plugin/plugin.json'))
  manifest.name = 'antiburn-dev'
  manifest.displayName = 'antiburn (Dev)'
  manifest.userConfig = {
    usageUiEnabled: {
      type: 'boolean',
      title: 'Show usage above the prompt',
      description:
        'Show usage UI and read usage automatically. Explicit usage commands remain available when disabled. Reload after changing this option.',
      default: true,
    },
    startupNoticeEnabled: {
      type: 'boolean',
      title: 'Debug: show CLI discovery at startup',
      description:
        'Show the local CLI availability notice once per session. This does not affect commands or live checks. Reload after changing this option.',
      default: false,
    },
  }
  files.set('.claude-plugin/plugin.json', json(manifest))
  files.set('LICENSE', containedFile(root, 'LICENSE'))
  if (tests)
    for (const [path, bytes] of releaseFiles(root)) files.set(`release-reference/${path}`, bytes)
  const combinedFiles = new Map(
    [...files].map(([path, bytes]) => [`plugins/antiburn-dev/${path}`, bytes]),
  )
  assertImports(combinedFiles, ['node:fs/promises', 'node:fs', 'node:path'])
  return files
}

export function assertValidation(report, type = 'plugin') {
  if (!report.success || report.manifest?.type !== type)
    throw new Error('Host validation failed or returned an unexpected report')

  for (const entry of [report.manifest, ...(report.contents ?? [])]) {
    if ((entry.errors ?? []).length)
      throw new Error(`Validation errors: ${JSON.stringify(entry.errors)}`)
    if ((entry.warnings ?? []).length)
      throw new Error(`Unexpected validation warning: ${JSON.stringify(entry.warnings)}`)
  }
}

export function assertCapabilities(report) {
  if (!report.success) throw new Error('Host validation failed')
  const inventories = (report.contents ?? [])
    .flatMap((entry) => entry.notes ?? [])
    .filter((note) => note.includes(' calls:'))
  if (inventories.length === 0) throw new Error('Host validation did not report a call inventory')
  const allowed = new Set([
    '$.clock.now',
    '$.command.register',
    '$.ui.log',
    '$.session.usage',
    '$.state.get',
    '$.state.set',
    '$.ui.resolve',
    '$.antiburn.describe',
  ])
  const accountOnly = new Set(['$.session.authorize', '$.http.fetch'])
  const websiteOnly = new Set(['$.env.get', '$.process.run', '$.ui.status'])
  const calls = inventories.flatMap((note) =>
    [
      ...note.matchAll(/(\$\.[A-Za-z_]+\.[A-Za-z_]+)(.*?)(?=,\s*\$\.[A-Za-z_]+\.[A-Za-z_]+|$)/g),
    ].map((match) => ({ name: match[1], attribution: match[2].trim() })),
  )
  if (!calls.some(({ name }) => name === '$.session.usage'))
    throw new Error('Usage call is missing from the host inventory')
  for (const { name, attribution } of calls) {
    if (websiteOnly.has(name)) {
      if (attribution !== '(via openTerminalWebsite)')
        throw new Error(`Unapproved release capability: ${name} ${attribution}`)
    } else if (accountOnly.has(name)) {
      if (attribution !== '(via fetchAccountUsage)')
        throw new Error(`Unapproved release capability: ${name} ${attribution}`)
    } else if (!allowed.has(name)) {
      throw new Error(`Unapproved release capability: ${name}`)
    }
  }
}

export function assemble(root = ROOT, mode = 'release') {
  if (mode === 'dev' || mode === 'dev-test') {
    const destination = join(root, LOCAL, mode)
    const developmentFiles = devFiles(root, mode === 'dev-test')
    writeTree(root, join(destination, 'plugins/antiburn-dev'), developmentFiles)
    const marketplace = {
      name: 'antiburn-local-dev',
      owner: { name: 'antiburn' },
      metadata: {
        description: 'Local antiburn development plugins; do not publish.',
      },
      plugins: [
        {
          name: 'antiburn-dev',
          source: './plugins/antiburn-dev',
        },
      ],
    }
    containedDestination(root, join(destination, '.claude-plugin/marketplace.json'))
    mkdirSync(join(destination, '.claude-plugin'), { recursive: true })
    writeFileSync(join(destination, '.claude-plugin/marketplace.json'), json(marketplace))
    return destination
  }
  const files = releaseFiles(root)
  const destination = join(root, PUBLIC)
  if (mode === 'check') checkTree(destination, files)
  else if (mode === 'test') {
    checkTree(destination, files)
    for (const [source, target] of [
      [`${SOURCE}/release/hooks/register.test.ts`, 'hooks/register.test.ts'],
      [`${SOURCE}/release/hooks/usage-graphics.test.ts`, 'hooks/usage-graphics.test.ts'],
      [`${SOURCE}/hooks/usage.test.ts`, 'hooks/usage.test.ts'],
    ])
      files.set(target, containedFile(root, source))
    const tests = join(root, LOCAL, 'test')
    writeTree(root, tests, files)
    return tests
  } else writeTree(root, destination, files)
  return destination
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    const [flag, path, ...rest] = process.argv.slice(2)
    if (
      ['--validate-log', '--validate-marketplace-log'].includes(flag) &&
      path &&
      rest.length === 0
    ) {
      const report = JSON.parse(readFileSync(path, 'utf8'))
      assertValidation(report, flag === '--validate-marketplace-log' ? 'marketplace' : 'plugin')
      if (flag === '--validate-log') assertCapabilities(report)
      console.log('Strict validation and release capability policy passed')
    } else {
      if (path || (flag && !['--check', '--dev', '--dev-test', '--test'].includes(flag)))
        throw new Error(
          'Usage: assemble-claude-mod.mjs [--check | --dev | --dev-test | --test | --validate-log report.json | --validate-marketplace-log report.json]',
        )
      console.log(assemble(ROOT, flag?.slice(2) ?? 'release'))
    }
  } catch (error) {
    console.error(error instanceof Error ? error.message : String(error))
    process.exitCode = 1
  }
}
