import { stat, access } from 'node:fs/promises'
import { constants } from 'node:fs'
import path from 'node:path'

/** @typedef {import('./core.mjs').Availability} Availability */
/** @param {string} filename @param {string} platform */
async function executableFile(filename, platform) {
  try {
    const info = await stat(filename)
    if (!info.isFile()) return false
    if (platform !== 'win32' && !(info.mode & 0o111)) return false
    await access(filename, platform === 'win32' ? constants.F_OK : constants.X_OK)
    return true
  } catch (error) {
    const code = /** @type {NodeJS.ErrnoException} */ (error).code
    if (code === 'ENOENT' || code === 'ENOTDIR') return false
    throw error
  }
}

/**
 * @param {{env?: NodeJS.ProcessEnv, platform?: string, cwd?: string, timeoutMs?: number, probe?: (file: string, platform: string) => Promise<boolean>}} [options]
 * @returns {Promise<Availability>}
 */
export async function discover(options = {}) {
  const {
    env = process.env,
    platform = process.platform,
    cwd = process.cwd(),
    timeoutMs = 1500,
    probe = executableFile,
  } = options
  const paths = platform === 'win32' ? path.win32 : path.posix
  const override = env.ANTIBURN_EXECUTABLE
  const search = env.PATH
  const invalid = (/** @type {string} */ value) =>
    value.length > 8192 || /[\x00-\x1f\x7f]/.test(value) || /^(?:\/\/|\\\\)/.test(value)
  if (!['win32', 'darwin', 'linux', 'freebsd', 'openbsd'].includes(platform))
    return { state: 'unavailable' }
  if (override !== undefined && (!override || invalid(override) || !paths.isAbsolute(override)))
    return { state: 'unavailable' }
  if (override === undefined && (search === undefined || !search || search.length > 32768))
    return { state: 'unavailable' }
  const directories =
    override !== undefined ? [] : /** @type {string} */ (search).split(paths.delimiter)
  if (directories.length > 128 || directories.some((dir) => invalid(dir)))
    return { state: 'unavailable' }
  const extensions =
    platform === 'win32'
      ? (env.PATHEXT || '.COM;.EXE;.BAT;.CMD').split(';').map((ext) => ext.toUpperCase())
      : ['']
  if (
    extensions.length > 16 ||
    extensions.some((ext) => platform === 'win32' && !/^\.(COM|EXE|BAT|CMD)$/.test(ext))
  )
    return { state: 'unavailable' }
  if (
    platform === 'win32' &&
    override &&
    !extensions.includes(paths.extname(override).toUpperCase())
  )
    return { state: 'unavailable' }
  const candidates =
    override !== undefined
      ? [override]
      : directories.flatMap((dir) =>
          ['antiburn-cli', 'antiburn'].flatMap((name) =>
            extensions.map((ext) => paths.resolve(cwd, dir || '.', `${name}${ext}`)),
          ),
        )
  let expired = false
  /** @type {ReturnType<typeof setTimeout> | undefined} */
  let timer
  const deadline = new Promise(
    /** @param {(value: Availability) => void} resolve */ (resolve) => {
      timer = setTimeout(
        () => {
          expired = true
          resolve({ state: 'unavailable' })
        },
        Math.max(1, Math.min(1500, timeoutMs)),
      )
    },
  )
  const check = async () => {
    let failed = false
    for (const candidate of candidates) {
      if (expired) return /** @type {Availability} */ ({ state: 'unavailable' })
      try {
        if (await probe(candidate, platform))
          return /** @type {Availability} */ ({ state: 'found' })
      } catch {
        failed = true
      }
    }
    return /** @type {Availability} */ ({ state: failed ? 'unavailable' : 'not-found' })
  }
  try {
    return await Promise.race([check(), deadline])
  } finally {
    clearTimeout(timer)
  }
}
