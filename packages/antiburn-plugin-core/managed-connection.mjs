import {
  accessSync,
  closeSync,
  constants,
  fstatSync,
  openSync,
  readSync,
  realpathSync,
  statSync,
} from 'node:fs'
import { isAbsolute, win32 } from 'node:path'

// The desktop assembler sets this flag in its separate production artifact.
const MANAGED_BY_DESKTOP = false

/**
 * @param {{managed?:boolean,connectionUrl?:URL,platform?:string}} [options]
 * @returns {{managed:boolean,executable?:string}}
 */
export function managedExecutable(options = {}) {
  const managed = options.managed ?? MANAGED_BY_DESKTOP
  if (!managed) return { managed: false }
  let descriptor
  try {
    const url = options.connectionUrl ?? new URL('./connection.json', import.meta.url)
    descriptor = openSync(url, constants.O_RDONLY | constants.O_NOFOLLOW | constants.O_NONBLOCK)
    const metadata = fstatSync(descriptor)
    if (!metadata.isFile() || metadata.size > 16384) return { managed: true }
    const bytes = Buffer.alloc(16385)
    const count = readSync(descriptor, bytes, 0, bytes.length, 0)
    if (count > 16384) return { managed: true }
    const connection = JSON.parse(bytes.subarray(0, count).toString('utf8'))
    const platform = options.platform ?? process.platform
    const path = connection.executable
    const absolute = platform === 'win32' ? win32.isAbsolute : isAbsolute
    if (
      connection.schema !== 1 ||
      !Array.isArray(connection.deliveryProtocols) ||
      !connection.deliveryProtocols.includes('delivery-v1') ||
      typeof path !== 'string' ||
      !absolute(path) ||
      path.length > 8192 ||
      /[\x00-\x1f\x7f]/.test(path) ||
      !statSync(path).isFile()
    )
      return { managed: true }
    accessSync(path, platform === 'win32' ? constants.F_OK : constants.X_OK)
    return { managed: true, executable: realpathSync(path) }
  } catch {
    return { managed: true }
  } finally {
    if (descriptor !== undefined) closeSync(descriptor)
  }
}
