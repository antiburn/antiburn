import assert from 'node:assert/strict'
import { execFileSync } from 'node:child_process'
import {
  cpSync,
  existsSync,
  mkdtempSync,
  mkdirSync,
  readFileSync,
  rmSync,
  symlinkSync,
  writeFileSync,
} from 'node:fs'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { test } from 'node:test'
import { inflateSync } from 'node:zlib'
import {
  assemble,
  assertCapabilities,
  assertImports,
  assertValidation,
  checkTree,
  releaseFiles,
  rewriteDevSource,
} from './assemble-claude-mod.mjs'

const root = fileURLToPath(new URL('..', import.meta.url))
const report = (...calls) => ({
  success: true,
  contents: [{ notes: [`./register.tsx calls: ${calls.join(', ')}`] }],
})

function temporary(t) {
  const path = mkdtempSync(join(tmpdir(), 'antiburn-packaging-'))
  t.after(() => rmSync(path, { recursive: true, force: true }))
  return path
}

test('release allowlist is self-contained and preserves authoritative assets', () => {
  const files = releaseFiles(root)
  assert.equal(files.size, 26)
  assert.equal(files.has('skills/help/SKILL.md'), false)
  const commands = files.get('hooks/commands.ts').toString()
  assert.doesNotMatch(commands, /command\.list|antiburn-plugin|antiburn-mod/)
  assert.match(commands, /const COMMAND = "antiburn"/)
  assert.deepEqual(
    files.get('hooks/checking-status.ts'),
    readFileSync(join(root, 'plugins/antiburn-live/release/hooks/checking-status.ts')),
  )
  assert.deepEqual(files.get('LICENSE'), readFileSync(join(root, 'LICENSE')))
  assert.deepEqual(
    files.get('assets/icon.png'),
    readFileSync(join(root, 'plugins/antiburn-live/release/assets/icon.png')),
  )
  for (const path of files.keys())
    assert.doesNotMatch(
      path,
      /(?:checks|catalog|burn-controls|terminal-flame|fixtures|\.test\.|\.claude-plugin\/types)/,
    )
  const manifest = JSON.parse(files.get('.claude-plugin/plugin.json'))
  assert.equal(manifest.name, 'antiburn')
  assert.equal(manifest.version, '0.2.5')
  assert.equal(manifest.types, undefined)
  assert.equal(manifest.icon, './assets/icon.png')
  assert.equal(
    manifest.privacyPolicyUrl,
    'https://github.com/antiburn/antiburn/blob/main/plugins/antiburn/PRIVACY.md',
  )
  assert.equal(manifest.userConfig, undefined)
  assert.match(files.get('lib/product.mjs').toString(), /VERSION = '0\.2\.1'/)
  assert.deepEqual(JSON.parse(files.get('hooks/hooks.json')).modules, ['./quota-controller.tsx'])
  assert.ok(files.has('hooks/register.ts'))
  assert.ok(files.has('hooks/commands.ts'))
  assert.ok(files.has('hooks/quota-controller.tsx'))
  assert.ok(files.has('lib/product.mjs'))
  assert.doesNotMatch([...files.keys()].join('\n'), /(?:discovery|probe|checks|debug|\.test\.)/)
  const marketplace = JSON.parse(readFileSync(join(root, '.claude-plugin/marketplace.json')))
  assert.equal(marketplace.plugins.length, 1)
  assert.equal(marketplace.plugins[0].name, 'antiburn')
  assert.deepEqual(marketplace.plugins[0].source, {
    source: 'git-subdir',
    url: 'https://github.com/antiburn/antiburn.git',
    path: 'plugins/antiburn',
    sha: marketplace.plugins[0].source.sha,
  })
  assert.equal(marketplace.plugins[0].version, undefined)
  const source = marketplace.plugins[0].source
  assert.match(source.sha, /^[a-f0-9]{40}$/)
  assertImports(files)
  checkTree(join(root, 'plugins/antiburn'), files)
})

test('marketplace pin matches authoritative sources at its release commit', (t) => {
  const marketplace = JSON.parse(readFileSync(join(root, '.claude-plugin/marketplace.json')))
  const source = marketplace.plugins[0].source
  const snapshot = temporary(t)
  const archive = execFileSync(
    'git',
    [
      'archive',
      source.sha,
      'scripts/assemble-claude-mod.mjs',
      'plugins/antiburn-live',
      source.path,
      'integrations/antiburn/claude',
      'packages/antiburn-plugin-core',
      'LICENSE',
    ],
    { cwd: root, maxBuffer: 16 * 1024 * 1024 },
  )
  execFileSync('tar', ['-x', '-C', snapshot], { input: archive })
  const check = () =>
    execFileSync(process.execPath, ['scripts/assemble-claude-mod.mjs', '--check'], {
      cwd: snapshot,
      encoding: 'utf8',
      stdio: 'pipe',
    })
  check()
  const artifact = join(snapshot, source.path, 'hooks/usage-graphics.ts')
  writeFileSync(artifact, `${readFileSync(artifact, 'utf8')}\n// Changed artifact.\n`)
  assert.throws(check, (error) => {
    assert.match(error.stderr, /Artifact drift:.*hooks\/usage-graphics\.ts/)
    return true
  })
})

test('candidate plugin icon is a compact 512px PNG without metadata or trailing content', () => {
  function image(bytes) {
    assert.deepEqual(bytes.subarray(0, 8), Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]))
    const data = []
    let header
    let offset = 8
    while (offset < bytes.length) {
      const length = bytes.readUInt32BE(offset)
      const name = bytes.toString('ascii', offset + 4, offset + 8)
      const body = bytes.subarray(offset + 8, offset + 8 + length)
      assert.ok(['IHDR', 'IDAT', 'IEND', 'sRGB'].includes(name))
      if (name === 'IHDR') header = body
      if (name === 'IDAT') data.push(body)
      offset += length + 12
      if (name === 'IEND') {
        assert.equal(length, 0)
        assert.equal(offset, bytes.length)
        return { header, scanlines: inflateSync(Buffer.concat(data)) }
      }
    }
    assert.fail('PNG has no end chunk')
  }
  function pixels(header, scanlines) {
    const width = header.readUInt32BE(0)
    const height = header.readUInt32BE(4)
    const bytesPerPixel = 4
    const rowLength = width * bytesPerPixel
    assert.equal(scanlines.length, (rowLength + 1) * height)
    const output = Buffer.alloc(rowLength * height)
    let inputOffset = 0
    for (let row = 0; row < height; row += 1) {
      const filter = scanlines[inputOffset]
      inputOffset += 1
      const rowOffset = row * rowLength
      for (let column = 0; column < rowLength; column += 1) {
        const left = column >= bytesPerPixel ? output[rowOffset + column - bytesPerPixel] : 0
        const up = row > 0 ? output[rowOffset - rowLength + column] : 0
        const upperLeft =
          row > 0 && column >= bytesPerPixel
            ? output[rowOffset - rowLength + column - bytesPerPixel]
            : 0
        const paethBase = left + up - upperLeft
        const paethLeft = Math.abs(paethBase - left)
        const paethUp = Math.abs(paethBase - up)
        const paethUpperLeft = Math.abs(paethBase - upperLeft)
        const predictor = [
          0,
          left,
          up,
          Math.floor((left + up) / 2),
          paethLeft <= paethUp && paethLeft <= paethUpperLeft
            ? left
            : paethUp <= paethUpperLeft
              ? up
              : upperLeft,
        ][filter]
        assert.notEqual(predictor, undefined, `Unsupported PNG filter: ${filter}`)
        output[rowOffset + column] = (scanlines[inputOffset] + predictor) & 0xff
        inputOffset += 1
      }
    }
    return output
  }
  const plugin = readFileSync(join(root, 'plugins/antiburn-live/assets/icon.png'))
  assert.ok(
    plugin.length < 256 * 1024,
    'The entire PNG must fit below the printable-text limit',
  )
  const { header, scanlines } = image(plugin)
  assert.equal(header.readUInt32BE(0), 512)
  assert.equal(header.readUInt32BE(4), 512)
  assert.equal(header[8], 8)
  assert.equal(header[9], 6)
  const rgba = pixels(header, scanlines)
  assert.ok(
    rgba.some((value, index) => index % 4 === 3 && value < 255),
    'The icon must contain at least one transparent pixel',
  )
})

test('tree verification rejects stale, omitted, and unexpected files without writing', (t) => {
  const path = temporary(t)
  const expected = new Map([
    ['one', Buffer.from('first')],
    ['two', Buffer.from('second')],
  ])
  writeFileSync(join(path, 'one'), 'first')
  writeFileSync(join(path, 'two'), 'second')
  checkTree(path, expected)
  writeFileSync(join(path, 'one'), 'changed')
  rmSync(join(path, 'two'))
  writeFileSync(join(path, 'private'), 'unexpected')
  assert.throws(
    () => checkTree(path, expected),
    /"missing":\["two"\].*"extra":\["private"\].*"changed":\["one"\]/,
  )
  assert.equal(readFileSync(join(path, 'one'), 'utf8'), 'changed')
})

test('tree verification rejects symlinks', (t) => {
  const path = temporary(t)
  symlinkSync(resolve(root, 'LICENSE'), join(path, 'LICENSE'))
  assert.throws(() => checkTree(path, new Map()), /Symbolic link/)
})

test('source collection rejects a symlink in a parent directory', (t) => {
  const path = temporary(t)
  mkdirSync(join(path, 'plugins'))
  symlinkSync(join(root, 'plugins/antiburn-live'), join(path, 'plugins/antiburn-live'))
  assert.throws(() => releaseFiles(path), /Symbolic link/)
})

test('import verification rejects omitted modules, package imports, and path escapes', () => {
  for (const source of [
    "import x from './checks'",
    "import x from 'node:fs'",
    "import x from '../../usage'",
    "const x = import('./checks')",
    'const load = (path) => import(path)',
    'const load = (path) => import(\n  path\n)',
    "import x from './checks.js'",
    "import './checks'",
    "import { on } from 'claude-code'; import './missing'",
    "import { on } from 'claude-code'; import x from 'node:fs'",
    "export { On } from 'claude-code'; export { missing } from './missing'",
  ]) {
    assert.throws(
      () =>
        assertImports(
          new Map([
            ['hooks/register.tsx', Buffer.from(source)],
            ['usage.ts', Buffer.from('export {}')],
          ]),
        ),
      /import/i,
    )
  }
  assertImports(
    new Map([
      ['hooks/register.tsx', Buffer.from("import { on } from 'claude-code'; import './usage'")],
      ['hooks/usage.ts', Buffer.from('export {}')],
    ]),
  )
  assertImports(
    new Map([
      [
        'hooks/register.tsx',
        Buffer.from("import './runtime.mjs'; import './usage.js'; import('./runtime.mjs')"),
      ],
      ['hooks/runtime.mjs', Buffer.from('export {}')],
      ['hooks/usage.ts', Buffer.from('export {}')],
    ]),
  )
})

test('development rewrite isolates state and preserves explicit command registrations', () => {
  const source = `const owner = { plugin: 'antiburn-live' }
let commandName = 'antiburn-dev'
const registered = await $.command.register({ name: 'antiburn-dev' })
commandName = registered.command
const again = await $.command.register({ name: "antiburn-dev" })
commandName = again.command
const label = 'antiburn'`
  const rewritten = rewriteDevSource(source)
  assert.doesNotMatch(rewritten, /antiburn-live/)
  assert.equal(rewritten.match(/antiburn-dev/g).length, 4)
  assert.match(rewritten, /commandName = registered\.command/)
  assert.match(rewritten, /commandName = again\.command/)
  assert.match(rewritten, /const label = 'antiburn'/)
})

test('capability policy permits website activation APIs and rejects background and model actions', () => {
  assertCapabilities(
    report(
      '$.session.usage',
      '$.clock.now',
      '$.state.get',
      '$.command.register',
      '$.ui.log',
      '$.antiburn.describe',
      '$.env.get (via openTerminalWebsite)',
      '$.process.run (via openTerminalWebsite)',
      '$.ui.status (via openTerminalWebsite)',
    ),
  )
  for (const call of [
    '$.antiburnDevelopment.describe',
    '$.command.list',
    '$.clock.after',
    '$.clock.every',
    '$.http.get',
    '$.model.query',
    '$.prompt.fill',
    '$.prompt.submit',
    '$.session.compact',
  ]) {
    assert.throws(
      () => assertCapabilities(report('$.session.usage', call)),
      /Unapproved release capability/,
    )
  }
  assert.throws(() => assertCapabilities({ success: true }), /inventory/)
  assert.throws(() => assertCapabilities({ success: false }), /failed/)
  assert.throws(() => assertCapabilities(report('$.clock.now')), /Usage call/)
})

test('website capabilities require exclusive opener attribution for every occurrence', () => {
  for (const name of ['$.env.get', '$.process.run', '$.ui.status']) {
    for (const attribution of [
      '',
      ' (via sessionStart)',
      ' (via openTerminalWebsite, sessionStart)',
      ' (via sessionStart, openTerminalWebsite)',
      ' (via openTerminalWebsiteExtra)',
      ' (via openTerminalWebsite) (via sessionStart)',
    ]) {
      assert.throws(
        () => assertCapabilities(report('$.session.usage', `${name}${attribution}`)),
        /Unapproved release capability/,
      )
    }
    assert.throws(
      () =>
        assertCapabilities(report('$.session.usage', `${name} (via openTerminalWebsite)`, name)),
      /Unapproved release capability/,
    )
    const mixed = report('$.session.usage', `${name} (via openTerminalWebsite)`)
    mixed.contents.push({ notes: [`./other.ts calls: ${name}`] })
    assert.throws(() => assertCapabilities(mixed), /Unapproved release capability/)
  }
})

test('validation policy rejects every warning and error', () => {
  const warning = {
    path: 'name',
    code: null,
    message:
      'Plugin name "antiburn-mod" reads as one of Anthropic\'s own. Name it for what it does. If this is one of Anthropic\'s own plugins, validate the marketplace that lists it.',
  }
  const base = {
    success: true,
    manifest: { type: 'plugin', errors: [], warnings: [] },
    contents: [],
  }
  assertValidation({
    ...base,
    manifest: { type: 'plugin', errors: [], warnings: [] },
  })
  assert.throws(
    () =>
      assertValidation({
        ...base,
        manifest: { ...base.manifest, warnings: [warning] },
      }),
    /Unexpected validation warning/,
  )
  for (const changed of [
    { path: 'author' },
    { message: `${warning.message} Extra warning.` },
    { code: 'new-code' },
  ]) {
    assert.throws(
      () =>
        assertValidation({
          ...base,
          manifest: {
            ...base.manifest,
            warnings: [{ ...warning, ...changed }],
          },
        }),
      /Unexpected validation warning/,
    )
  }
  assert.throws(
    () => assertValidation({ ...base, contents: [{ errors: ['broken import'] }] }),
    /Validation errors/,
  )
  assert.throws(
    () => assertValidation({ ...base, contents: [{ warnings: [warning] }] }),
    /Unexpected validation warning/,
  )
  assert.throws(() => assertValidation({ ...base, success: false }), /Host validation failed/)
})

test('assembly is deterministic and isolates private, development, and test files', (t) => {
  const path = temporary(t)
  mkdirSync(join(path, 'plugins'), { recursive: true })
  cpSync(join(root, 'plugins/antiburn-live'), join(path, 'plugins/antiburn-live'), {
    recursive: true,
    filter: (source) => !source.includes('/.claude-plugin/types'),
  })
  const privateTypes = join(path, 'plugins/antiburn-live/.claude-plugin/types')
  mkdirSync(privateTypes, { recursive: true })
  writeFileSync(join(privateTypes, 'account.d.ts'), 'private fixture')
  mkdirSync(join(path, 'apps/desktop/src-tauri/icons'), { recursive: true })
  cpSync(
    join(root, 'apps/desktop/src-tauri/icons/512x512.png'),
    join(path, 'apps/desktop/src-tauri/icons/icon.png'),
  )
  cpSync(join(root, 'LICENSE'), join(path, 'LICENSE'))
  mkdirSync(join(path, 'integrations/antiburn/claude'), { recursive: true })
  cpSync(join(root, 'integrations/antiburn/claude'), join(path, 'integrations/antiburn/claude'), {
    recursive: true,
  })
  mkdirSync(join(path, 'packages/antiburn-plugin-core'), { recursive: true })
  for (const file of [
    'product.mjs',
    'core.mjs',
    'discovery.mjs',
    'probe.mjs',
    'session-checks-validation.mjs',
  ])
    cpSync(
      join(root, 'packages/antiburn-plugin-core', file),
      join(path, 'packages/antiburn-plugin-core', file),
    )
  const destination = assemble(path)
  const expected = releaseFiles(path)
  checkTree(destination, expected)
  assemble(path)
  checkTree(destination, expected)
  execFileSync('git', ['init', '--quiet'], { cwd: path, stdio: 'ignore' })
  execFileSync('git', ['add', 'plugins/antiburn'], {
    cwd: path,
    stdio: 'ignore',
  })
  rmSync(join(path, '.agent-artifacts/assembler-ownership'), {
    recursive: true,
  })
  const sourceReadme = join(path, 'plugins/antiburn-live/release/README.md')
  const originalReadme = readFileSync(sourceReadme)
  writeFileSync(sourceReadme, Buffer.concat([originalReadme, Buffer.from('\nSource update.\n')]))
  assemble(path)
  checkTree(destination, releaseFiles(path))
  writeFileSync(sourceReadme, originalReadme)
  assemble(path)
  checkTree(destination, expected)
  writeFileSync(join(destination, 'README.md'), 'user edit')
  assert.throws(() => assemble(path), /modified or unowned output/)
  writeFileSync(join(destination, 'README.md'), expected.get('README.md'))
  assemble(path)
  checkTree(destination, expected)
  assert.equal(existsSync(join(destination, '.claude-plugin/types')), false)
  assert.equal(existsSync(join(destination, 'hooks/checks.ts')), false)
  const devRoot = assemble(path, 'dev')
  const plugin = join(devRoot, 'plugins/antiburn-dev')
  const devManifest = JSON.parse(readFileSync(join(plugin, '.claude-plugin/plugin.json'), 'utf8'))
  assert.equal(devManifest.name, 'antiburn-dev')
  assert.equal(devManifest.userConfig.usageUiEnabled.default, true)
  assert.equal(devManifest.userConfig.startupNoticeEnabled.default, false)
  assert.equal(existsSync(join(plugin, 'release')), false)
  assert.equal(existsSync(join(plugin, '.claude-plugin/types')), false)
  assert.equal(existsSync(join(plugin, 'hooks/register.test.ts')), false)
  assert.equal(existsSync(join(plugin, 'hooks/checks.ts')), true)
  assert.equal(existsSync(join(plugin, 'lib/session-checks-validation.mjs')), true)
  const dev = readFileSync(join(plugin, 'hooks/register.tsx'), 'utf8')
  assert.doesNotMatch(dev, /plugin: 'antiburn-live'|name: 'antiburn'/)
  assert.match(dev, /commandName = registered.command/)
  const devTestRoot = assemble(path, 'dev-test')
  const devTestRelease = join(devTestRoot, 'plugins/antiburn-dev/release-reference')
  for (const [name, content] of expected)
    assert.deepEqual(readFileSync(join(devTestRelease, name)), content)
  const testRoot = assemble(path, 'test')
  assert.equal(existsSync(join(testRoot, 'hooks/register.test.ts')), true)
  for (const [name, content] of expected)
    assert.deepEqual(readFileSync(join(testRoot, name)), content)
  rmSync(join(path, '.agent-artifacts'), { recursive: true })
  const external = temporary(t)
  symlinkSync(external, join(path, '.agent-artifacts'))
  assert.throws(() => assemble(path, 'dev'), /Symbolic link/)
  assert.equal(existsSync(join(external, 'claude-mod')), false)
})

test('development rewrite preserves explicit commands and brand URLs', () => {
  const input =
    'Run /antiburn-dev demo off. ` /antiburn-dev ` https://antiburn.com https://github.com/antiburn/antiburn'
  const result = rewriteDevSource(input)
  assert.equal(
    result,
    'Run /antiburn-dev demo off. ` /antiburn-dev ` https://antiburn.com https://github.com/antiburn/antiburn',
  )
})

test('account capabilities require exclusive fetchAccountUsage attribution', () => {
  for (const name of ['$.session.authorize', '$.http.fetch']) {
    assertCapabilities(report('$.session.usage', `${name} (via fetchAccountUsage)`))
    for (const attribution of [
      '',
      ' (via renderUsage)',
      ' (via fetchAccountUsage, readUsage)',
      ' (via readUsage, fetchAccountUsage)',
      ' (via fetchAccountUsageExtra)',
      ' (via fetchAccountUsage) (via renderUsage)',
    ]) {
      assert.throws(
        () => assertCapabilities(report('$.session.usage', `${name}${attribution}`)),
        /Unapproved release capability/,
      )
    }
    assert.throws(
      () => assertCapabilities(report('$.session.usage', `${name} (via fetchAccountUsage)`, name)),
      /Unapproved release capability/,
    )
  }
})

test('public hook helpers keep capability and registration arguments in their module', () => {
  const files = releaseFiles(root)
  let checked = 0
  for (const [path, bytes] of files) {
    if (!path.startsWith('hooks/') || !/\.tsx?$/.test(path)) continue
    const source = bytes.toString()
    const local = new Set([...source.matchAll(/\bfunction\s+(\w+)\s*\(/g)].map((match) => match[1]))
    for (const match of source.matchAll(/\b(\w+)\(\s*(\$|on)(?=[\s,)])/g)) {
      if (match[1] === 'read' || match[1] === 'update') {
        assert.match(source, /import\s*\{[^}]*\bread\b[^}]*\bupdate\b[^}]*\}\s*from "claude-code"/)
        continue
      }
      assert.ok(local.has(match[1]), `${path}: ${match[1]} forwards ${match[2]} outside its module`)
      checked++
    }
  }
  assert.ok(checked >= 8)
})

test('both artifacts package the same meter graphics and flex layout without fixture policy', (t) => {
  const path = temporary(t)
  cpSync(join(root, 'plugins'), join(path, 'plugins'), { recursive: true })
  cpSync(join(root, 'packages'), join(path, 'packages'), { recursive: true })
  cpSync(join(root, 'integrations'), join(path, 'integrations'), {
    recursive: true,
  })
  cpSync(join(root, 'LICENSE'), join(path, 'LICENSE'))
  const release = releaseFiles(path)
  const dev = assemble(path, 'dev')
  for (const file of ['usage-meter-graphics.ts', 'usage-meter-band.tsx']) {
    assert.deepEqual(
      release.get(`hooks/${file}`),
      readFileSync(join(dev, 'plugins/antiburn-dev/hooks', file)),
    )
    assert.doesNotMatch(
      release.get(`hooks/${file}`).toString(),
      /usage-fixtures|usageFixture|USAGE_FIXTURES/,
    )
  }
  for (const [entry, expression] of [
    ['plugins/antiburn-live/release/hooks/quota-controller.tsx', /renderUsageMeterBand\(/],
    ['plugins/antiburn-live/hooks/register-usage-bands.tsx', /renderUsageMeterBand\(/],
    ['plugins/antiburn-live/hooks/register-usage-view.tsx', /responsiveMeterSvg\(/],
  ])
    assert.match(readFileSync(join(root, entry), 'utf8'), expression)
  assert.match(
    release.get('hooks/quota-controller.tsx').toString(),
    /responsiveMeterSvg\(window, now\)/,
  )
  for (const file of release.keys())
    assert.doesNotMatch(file, /fixtures|debug|burn-controls|register-usage-view/)
})
