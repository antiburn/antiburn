import { createHash } from 'node:crypto'
import { lstat, readFile, readdir } from 'node:fs/promises'
import { dirname, join, resolve } from 'node:path'

export const hash = (value) => createHash('sha256').update(value).digest('hex')

export async function rejectSymlinkParents(destination) {
  for (let current = resolve(destination); ;) {
    try {
      if ((await lstat(current)).isSymbolicLink()) throw new Error('Output symlink refused')
    } catch (error) {
      if (!(error instanceof Error && 'code' in error && error.code === 'ENOENT')) throw error
    }
    const parent = dirname(current)
    if (parent === current) return
    current = parent
  }
}

export async function inventory(directory, prefix = '') {
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

export async function verifyArtifactFiles(directory, files, excluded) {
  await rejectSymlinkParents(directory)
  const actual = (await inventory(directory)).filter((name) => !excluded.includes(name))
  if (JSON.stringify(actual) !== JSON.stringify(files.map(({ path }) => path)))
    throw new Error('Artifact inventory mismatch')
  for (const entry of files)
    if (hash(await readFile(join(directory, entry.path))) !== entry.sha256)
      throw new Error(`Artifact hash mismatch: ${entry.path}`)
}
