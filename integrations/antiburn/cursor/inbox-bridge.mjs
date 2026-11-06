import {
  closeSync,
  constants,
  existsSync,
  fstatSync,
  lstatSync,
  mkdirSync,
  mkdtempSync,
  openSync,
  readSync,
  readdirSync,
  realpathSync,
  renameSync,
  rmdirSync,
  unlinkSync,
  writeFileSync,
} from 'node:fs'
import { createHash, randomBytes } from 'node:crypto'
import { basename, dirname, isAbsolute, join, resolve } from 'node:path'

const MAX_ENROLLMENTS = 32
const MAX_REFERENCES = 16
const MAX_REFERENCE_FILES = 64
const MAX_BYTES = 128 * 1024
const MAX_ROOT_LENGTH = 4096
const MAX_DIRECTORY_ENTRIES = 512
const TTL = 60_000
const ID = /^[A-Za-z0-9_-]{1,128}$/
const ENROLLMENT = /^[a-f0-9]{32}$/
const MARKER = 'antiburn-inbox.json'
const MANIFEST = 'enrollment.json'
const MANIFEST_FILE = /^enrollment\.json(?:\.[a-f0-9]{16}\.tmp)?$/
const REFERENCE_FILE = /^[a-f0-9]{64}\.json(?:\.[a-f0-9]{16}\.tmp)?$/

function validBase(base) {
  if (basename(base) !== 'inbox-v1' || !directory(base)) return false
  const marker = readJson(join(base, MARKER))
  return marker?.protocol_version === 1 && marker.kind === 'antiburn-cursor-inbox-references'
}

function privateEntry(stat, directory) {
  return (
    process.platform !== 'win32' &&
    typeof process.getuid === 'function' &&
    stat.uid === process.getuid() &&
    (stat.mode & 0o077) === 0 &&
    (directory ? stat.isDirectory() : stat.isFile() && stat.nlink === 1)
  )
}

function directory(path) {
  return (
    isAbsolute(path) &&
    realpathSync(path) === resolve(path) &&
    privateEntry(lstatSync(path), true)
  )
}

function readJson(path) {
  let fd
  try {
    fd = openSync(path, constants.O_RDONLY | constants.O_NOFOLLOW | constants.O_NONBLOCK)
    const stat = fstatSync(fd)
    if (!privateEntry(stat, false) || stat.size > MAX_BYTES) return undefined
    const buffer = Buffer.alloc(MAX_BYTES + 1)
    let total = 0
    while (total < buffer.length) {
      const length = readSync(fd, buffer, total, buffer.length - total, null)
      if (length === 0) break
      total += length
    }
    if (total > MAX_BYTES) return undefined
    const text = buffer.subarray(0, total).toString('utf8')
    return JSON.parse(text)
  } catch {
    return undefined
  } finally {
    if (fd !== undefined) closeSync(fd)
  }
}

function writeJson(path, value) {
  if (existsSync(path) && !privateEntry(lstatSync(path), false))
    throw new Error('Unsafe Inbox file')
  const serialized = JSON.stringify(value)
  if (Buffer.byteLength(serialized) > MAX_BYTES) throw new Error('Inbox file limit reached')
  const temporary = `${path}.${randomBytes(8).toString('hex')}.tmp`
  try {
    writeFileSync(temporary, serialized, { flag: 'wx', mode: 0o600 })
    renameSync(temporary, path)
  } finally {
    if (existsSync(temporary)) unlinkSync(temporary)
  }
}

function hasErrorCode(error, code) {
  return error instanceof Error && 'code' in error && error.code === code
}

function unlinkIfPresent(path) {
  try {
    unlinkSync(path)
  } catch (error) {
    if (!hasErrorCode(error, 'ENOENT')) throw error
  }
}

function validRoot(root) {
  return (
    typeof root === 'string' &&
    root.length <= MAX_ROOT_LENGTH &&
    isAbsolute(root) &&
    !/[\x00-\x1f\x7f]/.test(root)
  )
}

/** Resolve only explicit local roots. Empty and remote workspaces are unavailable. */
export function normalizeInboxRoots(roots) {
  if (!Array.isArray(roots) || roots.length < 1 || roots.length > 8) return undefined
  try {
    const normalized = roots.map((root) => {
      if (!validRoot(root)) throw new Error('Invalid workspace root')
      const canonical = realpathSync(root)
      if (!validRoot(canonical) || !lstatSync(canonical).isDirectory())
        throw new Error('Invalid workspace directory')
      return canonical
    })
    if (new Set(normalized).size !== normalized.length) return undefined
    return normalized.sort()
  } catch {
    return undefined
  }
}

function sameRoots(left, right) {
  return (
    Array.isArray(left) &&
    left.length === right.length &&
    left.every((root, index) => root === right[index])
  )
}

function validManifest(manifest, id, roots, now) {
  return (
    manifest?.protocol_version === 1 &&
    manifest.enrollmentId === id &&
    sameRoots(manifest.workspaceRoots, roots) &&
    Number.isSafeInteger(manifest.expiresAt) &&
    manifest.expiresAt > now &&
    manifest.expiresAt <= now + TTL
  )
}

function initializeInboxBase(base) {
  if (existsSync(base)) {
    if (!directory(base)) throw new Error('Inbox directory must be private and owned')
    if (validBase(base)) return
    if (readdirSync(base).length !== 0) {
      if (validBase(base)) return
      throw new Error('Inbox directory marker is unavailable')
    }
  }
  const temporary = mkdtempSync(`${base}.`)
  try {
    writeJson(join(temporary, MARKER), {
      protocol_version: 1,
      kind: 'antiburn-cursor-inbox-references',
    })
    try {
      renameSync(temporary, base)
    } catch (error) {
      if (!hasErrorCode(error, 'EEXIST') && !hasErrorCode(error, 'ENOTEMPTY')) throw error
    }
    if (!validBase(base)) throw new Error('Inbox directory marker is unavailable')
  } finally {
    if (existsSync(temporary)) {
      unlinkIfPresent(join(temporary, MARKER))
      rmdirSync(temporary)
    }
  }
}

/** Create an explicit window enrollment. The hook never calls this function. */
export function enrollInbox(base, roots, now = Date.now()) {
  const normalized = normalizeInboxRoots(roots)
  if (!normalized || process.platform === 'win32' || typeof process.getuid !== 'function')
    throw new Error('Inbox needs a local POSIX workspace')
  if (!isAbsolute(base) || basename(base) !== 'inbox-v1')
    throw new Error('Inbox path must be an absolute inbox-v1 directory')
  const parent = dirname(base)
  const parentStat = lstatSync(parent)
  if (
    realpathSync(parent) !== resolve(parent) ||
    parentStat.uid !== process.getuid() ||
    (parentStat.mode & 0o022) !== 0
  )
    throw new Error('Inbox parent must be owned and must not allow other users to write')
  initializeInboxBase(base)
  const entries = readdirSync(base)
  if (entries.length > MAX_DIRECTORY_ENTRIES) throw new Error('Inbox directory limit reached')
  let active = 0
  for (const id of entries.filter((entry) => ENROLLMENT.test(entry))) {
    try {
      const path = join(base, id)
      if (!directory(path)) continue
      const manifestPath = join(path, MANIFEST)
      const manifest = readJson(manifestPath)
      if (
        manifest?.protocol_version === 1 &&
        manifest.enrollmentId === id &&
        Number.isSafeInteger(manifest.expiresAt)
      ) {
        if (manifest.expiresAt <= now) disposeInboxEnrollment({ base, id })
        else if (manifest.expiresAt <= now + TTL) active += 1
      } else if (!existsSync(manifestPath) && lstatSync(path).mtimeMs <= now - TTL) {
        disposeInboxEnrollment({ base, id })
      }
    } catch {
      continue
    }
  }
  if (readdirSync(base).length >= MAX_DIRECTORY_ENTRIES)
    throw new Error('Inbox directory limit reached')
  if (active >= MAX_ENROLLMENTS) throw new Error('Inbox active enrollment limit reached')
  const id = randomBytes(16).toString('hex')
  const path = join(base, id)
  mkdirSync(path, { mode: 0o700 })
  mkdirSync(join(path, 'references'), { mode: 0o700 })
  const manifest = {
    protocol_version: 1,
    enrollmentId: id,
    workspaceRoots: normalized,
    expiresAt: now + TTL,
  }
  writeJson(join(path, MANIFEST), manifest)
  return { base, id, roots: normalized }
}

function enrollmentPath(enrollment) {
  if (!ENROLLMENT.test(enrollment.id) || !validBase(enrollment.base))
    throw new Error('Unsafe enrollment')
  const path = join(enrollment.base, enrollment.id)
  if (!directory(path) || !directory(join(path, 'references')))
    throw new Error('Unsafe enrollment directory')
  return path
}

export function renewInbox(enrollment, now = Date.now()) {
  const path = enrollmentPath(enrollment)
  writeJson(join(path, MANIFEST), {
    protocol_version: 1,
    enrollmentId: enrollment.id,
    workspaceRoots: enrollment.roots,
    expiresAt: now + TTL,
  })
}

export function disposeInboxEnrollment(enrollment) {
  try {
    if (!ENROLLMENT.test(enrollment.id) || !validBase(enrollment.base)) return
    const path = join(enrollment.base, enrollment.id)
    if (!directory(path)) return
    const entries = readdirSync(path)
    if (entries.length > MAX_REFERENCE_FILES) return
    const manifests = entries.filter((name) => name !== 'references')
    if (
      manifests.some(
        (name) =>
          !MANIFEST_FILE.test(name) || !privateEntry(lstatSync(join(path, name)), false),
      )
    )
      return
    const references = join(path, 'references')
    const hasReferences = entries.includes('references')
    if (hasReferences && !directory(references)) return
    const files = hasReferences ? readdirSync(references) : []
    if (
      files.length > MAX_REFERENCE_FILES ||
      files.some(
        (name) =>
          !REFERENCE_FILE.test(name) || !privateEntry(lstatSync(join(references, name)), false),
      )
    )
      return
    unlinkIfPresent(join(path, MANIFEST))
    for (const name of manifests) {
      if (name !== MANIFEST) unlinkIfPresent(join(path, name))
    }
    for (const name of files) unlinkIfPresent(join(references, name))
    if (hasReferences) rmdirSync(references)
    rmdirSync(path)
  } catch {
    return
  }
}

/** Publish identity references only. Missing or expired enrollment is silent. */
export function publishInboxReference(base, input, now = Date.now()) {
  if (
    !input ||
    input.hook_event_name !== 'postToolUse' ||
    input.subagent_id !== undefined ||
    input.parent_conversation_id !== undefined ||
    typeof input.conversation_id !== 'string' ||
    typeof input.generation_id !== 'string' ||
    !ID.test(input.conversation_id) ||
    !ID.test(input.generation_id)
  )
    return 0
  const roots = normalizeInboxRoots(input.workspace_roots)
  if (!roots) return 0
  let published = 0
  try {
    if (!validBase(base)) return 0
    const ids = readdirSync(base)
    if (ids.length > MAX_DIRECTORY_ENTRIES) return 0
    for (const id of ids.filter((entry) => ENROLLMENT.test(entry))) {
      if (published >= MAX_ENROLLMENTS) break
      try {
        const enrollment = { base, id, roots }
        const path = enrollmentPath(enrollment)
        if (!validManifest(readJson(join(path, MANIFEST)), id, roots, now)) continue
        const references = join(path, 'references')
        const key = createHash('sha256').update(input.conversation_id).digest('hex')
        const file = `${key}.json`
        const entries = readdirSync(references)
        if (entries.length > MAX_REFERENCE_FILES) continue
        if (!entries.includes(file) && entries.length >= MAX_REFERENCES) {
          const previous = entries.map((name) => {
            if (!/^[a-f0-9]{64}\.json$/.test(name)) return undefined
            const value = readJson(join(references, name))
            return value?.enrollmentId === id && Number.isSafeInteger(value.observedAt)
              ? { name, observedAt: value.observedAt }
              : undefined
          })
          const complete = previous.filter((value) => value !== undefined)
          if (complete.length !== previous.length) continue
          complete.sort((left, right) => left.observedAt - right.observedAt)
          for (const previous of complete.slice(0, complete.length - MAX_REFERENCES + 1))
            unlinkIfPresent(join(references, previous.name))
        }
        writeJson(join(references, file), {
          protocol_version: 1,
          enrollmentId: id,
          sessionId: input.conversation_id,
          workspaceRoots: roots,
          observedAt: now,
        })
        published += 1
      } catch {
        continue
      }
    }
  } catch {
    return published
  }
  return published
}

export function readInboxReferences(enrollment, now = Date.now()) {
  try {
    const path = enrollmentPath(enrollment)
    if (!validManifest(readJson(join(path, MANIFEST)), enrollment.id, enrollment.roots, now))
      return []
    const dir = join(path, 'references')
    const files = readdirSync(dir)
    if (files.length > MAX_REFERENCE_FILES) return []
    return files
      .filter((name) => /^[a-f0-9]{64}\.json$/.test(name))
      .flatMap((name) => {
        const value = readJson(join(dir, name))
        if (
          value?.protocol_version !== 1 ||
          value.enrollmentId !== enrollment.id ||
          !ID.test(value.sessionId ?? '') ||
          !sameRoots(value.workspaceRoots, enrollment.roots) ||
          !Number.isSafeInteger(value.observedAt) ||
          value.observedAt > now ||
          now - value.observedAt > 24 * 60 * 60_000 ||
          name !== `${createHash('sha256').update(value.sessionId).digest('hex')}.json`
        )
          return []
        return [value]
      })
      .sort((left, right) => right.observedAt - left.observedAt)
      .slice(0, MAX_REFERENCES)
      .map((value) => value.sessionId)
  } catch {
    return []
  }
}
