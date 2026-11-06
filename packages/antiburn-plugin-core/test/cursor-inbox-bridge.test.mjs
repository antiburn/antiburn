import assert from 'node:assert/strict'
import { execFileSync } from 'node:child_process'
import { test } from 'node:test'
import fs from 'node:fs'
import {
  chmodSync,
  existsSync,
  lstatSync,
  mkdirSync,
  mkdtempSync,
  readdirSync,
  readFileSync,
  realpathSync,
  rmSync,
  symlinkSync,
  utimesSync,
  writeFileSync,
} from 'node:fs'
import { syncBuiltinESMExports } from 'node:module'
import { join } from 'node:path'
import { tmpdir } from 'node:os'
import { createHash } from 'node:crypto'
import {
  enrollInbox,
  renewInbox,
  disposeInboxEnrollment,
  publishInboxReference,
  readInboxReferences,
  normalizeInboxRoots,
} from '../../../integrations/antiburn/cursor/inbox-bridge.mjs'

function fixture(t) {
  const root = realpathSync(mkdtempSync(join(tmpdir(), 'ab-inbox-')))
  t.after(() => rmSync(root, { recursive: true, force: true }))
  const workspace = join(root, 'workspace')
  mkdirSync(workspace)
  return { root, workspace, base: join(root, 'inbox-v1') }
}
const payload = (workspace, id = 'conversation-1') => ({
  hook_event_name: 'postToolUse',
  conversation_id: id,
  generation_id: 'generation-1',
  workspace_roots: [workspace],
})

test('exact root sets isolate workspaces while enrolled windows fan out explicitly', (t) => {
  const f = fixture(t)
  const one = enrollInbox(f.base, [f.workspace], 1000)
  const two = enrollInbox(f.base, [f.workspace], 1000)
  const other = join(f.root, 'other')
  mkdirSync(other)
  const third = enrollInbox(f.base, [other], 1000)
  assert.equal(publishInboxReference(f.base, payload(f.workspace), 1100), 2)
  assert.deepEqual(readInboxReferences(one, 1100), ['conversation-1'])
  assert.deepEqual(readInboxReferences(two, 1100), ['conversation-1'])
  assert.deepEqual(readInboxReferences(third, 1100), [])
  assert.equal(
    publishInboxReference(
      f.base,
      { ...payload(f.workspace), workspace_roots: [f.workspace, other] },
      1100,
    ),
    0,
  )
  const text = readFileSync(
    join(f.base, one.id, 'references', readdirSync(join(f.base, one.id, 'references'))[0]),
    'utf8',
  )
  assert.deepEqual(Object.keys(JSON.parse(text)).sort(), [
    'enrollmentId',
    'observedAt',
    'protocol_version',
    'sessionId',
    'workspaceRoots',
  ])
})

test('a named pipe in the Inbox does not block reading valid references', (t) => {
  const f = fixture(t)
  const enrollment = enrollInbox(f.base, [f.workspace])
  assert.equal(publishInboxReference(f.base, payload(f.workspace)), 1)
  const fifo = join(f.base, enrollment.id, 'references', 'a'.repeat(64) + '.json')
  execFileSync('mkfifo', ['-m', '600', fifo])
  const bridge = new URL(
    '../../../integrations/antiburn/cursor/inbox-bridge.mjs',
    import.meta.url,
  )
  const output = execFileSync(
    process.execPath,
    [
      '--input-type=module',
      '-e',
      `
        const { readInboxReferences } = await import(process.argv[1])
        const enrollment = JSON.parse(process.argv[2])
        console.log(JSON.stringify(readInboxReferences(enrollment)))
      `,
      bridge.href,
      JSON.stringify(enrollment),
    ],
    { encoding: 'utf8', timeout: 3000 },
  )
  assert.deepEqual(JSON.parse(output), ['conversation-1'])
})

test('missing bridge, invalid identity, child ownership and absent generation remain silent', (t) => {
  const f = fixture(t)
  assert.equal(publishInboxReference(f.base, payload(f.workspace), 1000), 0)
  assert.equal(existsSync(f.base), false)
  const enrolled = enrollInbox(f.base, [f.workspace], 1000)
  for (const change of [
    { generation_id: undefined },
    { generation_id: '' },
    { subagent_id: 'child' },
    { parent_conversation_id: 'parent' },
    { conversation_id: '../other' },
    { conversation_id: 123 },
    { workspace_roots: [] },
    { hook_event_name: 'preToolUse' },
    { hook_event_name: undefined },
  ])
    assert.equal(publishInboxReference(f.base, { ...payload(f.workspace), ...change }, 1100), 0)
  assert.deepEqual(readInboxReferences(enrolled, 1100), [])
})

test('the reference hook cannot adopt an arbitrary private directory without an Enroll marker', (t) => {
  const f = fixture(t)
  mkdirSync(f.base, { mode: 0o700 })
  writeFileSync(join(f.base, 'unrelated'), 'untouched', { mode: 0o600 })
  assert.equal(publishInboxReference(f.base, payload(f.workspace), 1100), 0)
  assert.deepEqual(readdirSync(f.base), ['unrelated'])
  assert.throws(() => enrollInbox(f.base, [f.workspace], 1000), /marker/)
})

test('two initial enrollments both succeed when one publishes the Inbox while the other initializes it', (t) => {
  const f = fixture(t)
  const rename = fs.renameSync
  let second
  let initializingSecond = false
  t.mock.method(fs, 'renameSync', (source, target) => {
    if (
      !initializingSecond &&
      (target === f.base || target === join(f.base, 'antiburn-inbox.json'))
    ) {
      initializingSecond = true
      second = enrollInbox(f.base, [f.workspace], 1000)
    }
    return rename(source, target)
  })
  syncBuiltinESMExports()
  let first
  try {
    first = enrollInbox(f.base, [f.workspace], 1000)
  } finally {
    t.mock.restoreAll()
    syncBuiltinESMExports()
  }
  assert.ok(second)
  assert.notEqual(first.id, second.id)
  assert.equal(publishInboxReference(f.base, payload(f.workspace), 1100), 2)
})

test('expiry, renewal and cleanup bound enrollment lifecycle', (t) => {
  const f = fixture(t)
  const enrolled = enrollInbox(f.base, [f.workspace], 1000)
  assert.equal(publishInboxReference(f.base, payload(f.workspace), 61_001), 0)
  renewInbox(enrolled, 62_000)
  assert.equal(publishInboxReference(f.base, payload(f.workspace), 62_100), 1)
  disposeInboxEnrollment(enrolled)
  assert.equal(existsSync(join(f.base, enrolled.id)), false)
  assert.deepEqual(readInboxReferences(enrolled, 62_100), [])
})

test('32 expired enrollments cannot starve a new live window', (t) => {
  const f = fixture(t)
  for (let index = 0; index < 32; index++) enrollInbox(f.base, [f.workspace], 1000)
  const live = enrollInbox(f.base, [f.workspace], 70_000)
  assert.equal(publishInboxReference(f.base, payload(f.workspace), 70_100), 1)
  assert.deepEqual(readInboxReferences(live, 70_100), ['conversation-1'])
  assert.equal(readdirSync(f.base).length, 2)
})

test('expired renewals and interrupted cleanup leave no stale private enrollment directories', (t) => {
  const f = fixture(t)
  const expired = enrollInbox(f.base, [f.workspace], 1000)
  const expiredPath = join(f.base, expired.id)
  const interruptedRenewal = 'enrollment.json.0123456789abcdef.tmp'
  writeFileSync(join(expiredPath, interruptedRenewal), '{', { mode: 0o600 })

  const orphan = join(f.base, '1'.repeat(32))
  const unrelated = join(f.base, '2'.repeat(32))
  const fresh = join(f.base, '3'.repeat(32))
  for (const path of [orphan, unrelated, fresh]) mkdirSync(path, { mode: 0o700 })
  writeFileSync(join(orphan, interruptedRenewal), '{', { mode: 0o600 })
  writeFileSync(join(unrelated, 'untouched'), 'keep', { mode: 0o600 })
  const old = new Date(Date.now() - 120_000)
  for (const path of [orphan, unrelated]) utimesSync(path, old, old)

  const live = enrollInbox(f.base, [f.workspace])
  assert.deepEqual(
    readdirSync(f.base).sort(),
    ['2'.repeat(32), '3'.repeat(32), 'antiburn-inbox.json', live.id].sort(),
  )
  assert.equal(readFileSync(join(unrelated, 'untouched'), 'utf8'), 'keep')
})

test('a transient concurrent 17th reference still yields the newest 16 and publication repairs retention', (t) => {
  const f = fixture(t)
  const enrolled = enrollInbox(f.base, [f.workspace], 1000)
  for (let index = 0; index < 16; index++)
    publishInboxReference(f.base, payload(f.workspace, `session-${index}`), 1001 + index)
  const sessionId = 'concurrent-session'
  const dir = join(f.base, enrolled.id, 'references')
  writeFileSync(
    join(dir, `${createHash('sha256').update(sessionId).digest('hex')}.json`),
    JSON.stringify({
      protocol_version: 1,
      enrollmentId: enrolled.id,
      sessionId,
      workspaceRoots: enrolled.roots,
      observedAt: 1100,
    }),
    { mode: 0o600 },
  )
  const ids = readInboxReferences(enrolled, 1100)
  assert.equal(ids.length, 16)
  assert.ok(ids.includes(sessionId))
  assert.equal(ids.includes('session-0'), false)
  assert.equal(publishInboxReference(f.base, payload(f.workspace, 'new-session'), 1200), 1)
  assert.equal(readdirSync(dir).length, 16)
})

test('active enrollment count is capped without disabling existing windows', (t) => {
  const f = fixture(t)
  for (let index = 0; index < 32; index++) enrollInbox(f.base, [f.workspace], 1000)
  assert.throws(() => enrollInbox(f.base, [f.workspace], 1000), /active enrollment limit/)
  assert.equal(publishInboxReference(f.base, payload(f.workspace), 1100), 32)
})

test('17th session evicts the oldest reference and duplicate publication stays bounded', (t) => {
  const f = fixture(t)
  const enrolled = enrollInbox(f.base, [f.workspace], 1000)
  for (let index = 0; index < 17; index++)
    assert.equal(
      publishInboxReference(f.base, payload(f.workspace, `session-${index}`), 1001 + index),
      1,
    )
  assert.equal(publishInboxReference(f.base, payload(f.workspace, 'session-16'), 1100), 1)
  const ids = readInboxReferences(enrolled, 1100)
  assert.equal(ids.length, 16)
  assert.equal(ids.includes('session-0'), false)
  assert.equal(ids.includes('session-16'), true)
})

test('symlinks and nonprivate modes fail closed without touching targets', (t) => {
  const f = fixture(t)
  const enrolled = enrollInbox(f.base, [f.workspace], 1000)
  const manifest = join(f.base, enrolled.id, 'enrollment.json')
  chmodSync(manifest, 0o644)
  assert.equal(publishInboxReference(f.base, payload(f.workspace), 1100), 0)
  chmodSync(manifest, 0o600)
  const alias = join(f.root, 'alias')
  symlinkSync(f.base, alias)
  assert.equal(publishInboxReference(alias, payload(f.workspace), 1100), 0)
  const target = join(f.root, 'untouched')
  writeFileSync(target, 'unchanged', { mode: 0o600 })
  const references = join(f.base, enrolled.id, 'references')
  rmSync(references, { recursive: true })
  symlinkSync(target, references)
  assert.equal(publishInboxReference(f.base, payload(f.workspace), 1100), 0)
  disposeInboxEnrollment(enrolled)
  assert.equal(readFileSync(target, 'utf8'), 'unchanged')
  assert.equal(lstatSync(references).isSymbolicLink(), true)
})

test('oversized manifest is unavailable and root normalization rejects duplicates or relative paths', (t) => {
  const f = fixture(t)
  const enrolled = enrollInbox(f.base, [f.workspace], 1000)
  writeFileSync(join(f.base, enrolled.id, 'enrollment.json'), ' '.repeat(129 * 1024), {
    mode: 0o600,
  })
  assert.equal(publishInboxReference(f.base, payload(f.workspace), 1100), 0)
  assert.equal(normalizeInboxRoots([f.workspace, f.workspace]), undefined)
  assert.equal(normalizeInboxRoots(['relative']), undefined)
})

test(
  'eight long canonical roots round-trip through the manifest and reference',
  {
    skip: process.platform !== 'linux',
  },
  (t) => {
    const f = fixture(t)
    let parent = f.workspace
    for (let i = 0; i < 9; i++) {
      parent = join(parent, '"'.repeat(240))
      mkdirSync(parent)
    }
    const roots = Array.from({ length: 8 }, (_, index) => {
      const path = join(parent, String(index))
      mkdirSync(path)
      return path
    })
    assert.ok(Buffer.byteLength(JSON.stringify(roots)) > 16 * 1024)
    const enrollment = enrollInbox(f.base, roots, 1000)
    assert.equal(
      publishInboxReference(f.base, { ...payload(f.workspace), workspace_roots: roots }, 1100),
      1,
    )
    assert.deepEqual(readInboxReferences(enrollment, 1100), ['conversation-1'])
  },
)
