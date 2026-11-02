import { spawnSync } from 'node:child_process'
import { mkdtemp, readdir, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { dirname, join, resolve } from 'node:path'

const root = resolve(import.meta.dirname, '../..')
const own = join(root, 'packages/antiburn-plugin-core')
const integrations = await collect(join(root, 'integrations/antiburn'))
async function collect(directory) {
  const found = []
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    const path = join(directory, entry.name)
    if (entry.isDirectory()) found.push(...(await collect(path)))
    else if (/\.(mjs|json|ts)$/.test(path)) found.push(path)
  }
  return found
}
const files = [
  ...(await collect(own)),
  ...integrations.filter((path) => /\.(?:mjs|d\.ts)$/.test(path)),
  join(root, 'scripts/assemble-antiburn-core-plugin.mjs'),
  join(root, 'scripts/assemble-agent-integrations.mjs'),
  join(root, 'scripts/artifact-files.mjs'),
  join(root, 'scripts/artifact-files.test.mjs'),
  join(root, 'scripts/audit-antiburn-core-plugin.mjs'),
]
function run(command, args) {
  const result = spawnSync(command, args, { cwd: root, stdio: 'inherit' })
  if (result.error) throw result.error
  if (result.status) throw new Error(`Validation exited ${result.status}`)
}
function requiredValue(name) {
  const value = process.env[name]
  if (!value)
    throw new Error(
      `Set ${name} to the pinned SDK/type location; see packages/antiburn-plugin-core/README.md`,
    )
  return value
}
const mode = process.argv[2]
if (mode === 'lint')
  run(process.env.ANTIBURN_ESLINT || 'eslint', [
    '--no-ignore',
    '--config',
    join(own, 'eslint.config.mjs'),
    ...files.filter((path) => path.endsWith('.mjs')),
  ])
else if (mode === 'format')
  run(process.env.ANTIBURN_PRETTIER || 'prettier', [
    '--check',
    '--config',
    join(own, '.prettierrc.json'),
    ...files,
  ])
else if (mode === 'typecheck') {
  const required = [
    'ANTIBURN_CLAUDE_TYPES',
    'ANTIBURN_PI_TYPES',
    'ANTIBURN_OPENCODE_TYPES',
    'ANTIBURN_OPENCODE_SDK_ROOT',
    'ANTIBURN_NODE_TYPES',
  ]
  for (const name of required)
    if (!process.env[name])
      throw new Error(
        `Set ${name} to the pinned SDK/type location; see packages/antiburn-plugin-core/README.md`,
      )
  const temp = await mkdtemp(join(tmpdir(), 'antiburn-core-plugin-types-'))
  try {
    const config = {
      compilerOptions: {
        allowJs: true,
        checkJs: true,
        noEmit: true,
        strict: true,
        noImplicitAny: false,
        skipLibCheck: true,
        target: 'ES2022',
        module: 'NodeNext',
        moduleResolution: 'NodeNext',
        types: ['node'],
        typeRoots: [dirname(requiredValue('ANTIBURN_NODE_TYPES'))],
        paths: {
          'claude-code': [requiredValue('ANTIBURN_CLAUDE_TYPES')],
          '@earendil-works/pi-coding-agent': [requiredValue('ANTIBURN_PI_TYPES')],
          '@opencode-ai/plugin': [requiredValue('ANTIBURN_OPENCODE_TYPES')],
          '@opencode-ai/sdk': [
            join(requiredValue('ANTIBURN_OPENCODE_SDK_ROOT'), 'dist/index.d.ts'),
          ],
          '@opencode-ai/sdk/v2': [
            join(requiredValue('ANTIBURN_OPENCODE_SDK_ROOT'), 'dist/v2/index.d.ts'),
          ],
        },
      },
      files: files
        .filter((path) => !path.includes('/test/') && !path.endsWith('.json'))
        .concat(requiredValue('ANTIBURN_CLAUDE_TYPES')),
    }
    await writeFile(join(temp, 'tsconfig.json'), JSON.stringify(config))
    run(process.env.ANTIBURN_TSC || 'tsc', ['--project', join(temp, 'tsconfig.json')])
  } finally {
    await rm(temp, { recursive: true, force: true })
  }
} else throw new Error('Usage: node check.mjs lint|format|typecheck')
