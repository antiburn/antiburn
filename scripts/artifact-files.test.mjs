import assert from 'node:assert/strict'
import { mkdtemp, mkdir, readFile, realpath, rm, symlink, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { test } from 'node:test'
import {
  hash,
  inventory,
  rejectSymlinkParents,
  verifyArtifactFiles,
} from './artifact-files.mjs'

async function fixture(t) {
  const root = await realpath(await mkdtemp(join(tmpdir(), 'antiburn-artifact-files-')))
  t.after(() => rm(root, { recursive: true, force: true }))
  await mkdir(join(root, 'payload/lib'), { recursive: true })
  await writeFile(join(root, 'payload/lib/entry.mjs'), 'export const value = 1\n')
  await writeFile(join(root, 'payload/manifest.json'), '{}\n')
  return {
    root,
    output: join(root, 'payload'),
    files: [{ path: 'lib/entry.mjs', sha256: hash('export const value = 1\n') }],
  }
}

test('shared artifact verification checks exact inventory and hashes', async (t) => {
  const { output, files } = await fixture(t)
  assert.deepEqual(await inventory(output), ['lib/entry.mjs', 'manifest.json'])
  await verifyArtifactFiles(output, files, ['manifest.json'])
  await writeFile(join(output, 'lib/extra.mjs'), 'extra')
  await assert.rejects(verifyArtifactFiles(output, files, ['manifest.json']), /inventory/)
  await rm(join(output, 'lib/extra.mjs'))
  await writeFile(join(output, 'lib/entry.mjs'), 'user edit')
  await assert.rejects(verifyArtifactFiles(output, files, ['manifest.json']), /hash/)
  assert.equal(await readFile(join(output, 'lib/entry.mjs'), 'utf8'), 'user edit')
})

test('shared artifact verification rejects missing and redirected manifest entries', async (t) => {
  const { output, files } = await fixture(t)
  for (const path of ['missing.mjs', '../outside.mjs', '/outside.mjs'])
    await assert.rejects(
      verifyArtifactFiles(output, [{ ...files[0], path }], ['manifest.json']),
      /inventory/,
    )
  await assert.rejects(
    verifyArtifactFiles(output, [...files, ...files], ['manifest.json']),
    /inventory/,
  )
})

test('shared artifact checks reject nested links and redirected output parents', async (t) => {
  const { root, output, files } = await fixture(t)
  await symlink(join(output, 'lib/entry.mjs'), join(output, 'lib/link.mjs'))
  await assert.rejects(verifyArtifactFiles(output, files, ['manifest.json']), /symlink/)
  await rm(join(output, 'lib/link.mjs'))
  await symlink(output, join(root, 'redirected'))
  await assert.rejects(rejectSymlinkParents(join(root, 'redirected/missing')), /symlink/)
  await assert.rejects(
    verifyArtifactFiles(join(root, 'redirected'), files, ['manifest.json']),
    /symlink/,
  )
})
