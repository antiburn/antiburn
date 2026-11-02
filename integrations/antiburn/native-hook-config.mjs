import { isAbsolute, resolve } from 'node:path'
import { existsSync, realpathSync } from 'node:fs'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { parseArgs } from 'node:util'

const hosts = new Set(['codex', 'cursor'])
const supportedPlatforms = new Set(['darwin', 'linux'])

/** @param {string} value */
export function quotePosix(value) {
  return `'${value.replaceAll("'", `'"'"'`)}'`
}

/** @param {string} value */
export function quotePowerShell(value) {
  return `'${value.replaceAll("'", "''")}'`
}

/** @param {string[]} args */
export function parseSessionCheckConfigArgs(args) {
  const hostArguments = args.filter(
    (argument) => argument === '--host' || argument.startsWith('--host='),
  )
  if (hostArguments.length !== 1)
    throw new Error('Usage: node session-check-config.mjs --host codex|cursor')
  const { values } = parseArgs({
    args,
    options: { host: { type: 'string' } },
    strict: true,
    allowPositionals: false,
  })
  if (!values.host || !hosts.has(values.host))
    throw new Error('Usage: node session-check-config.mjs --host codex|cursor')
  return /** @type {'codex'|'cursor'} */ (values.host)
}

/** @param {{host:'codex'|'cursor',hookPath:string,platform?:string}} options */
export function renderSessionCheckConfig({ host, hookPath, platform = process.platform }) {
  if (!hosts.has(host)) throw new Error('Unsupported host. Choose codex or cursor.')
  if (!supportedPlatforms.has(platform))
    throw new Error(`Configuration preview is unsupported on ${platform}. Use macOS or Linux.`)
  if (!isAbsolute(hookPath)) throw new Error('The packaged hook path must be absolute.')

  const command = `node ${quotePosix(hookPath)}`
  if (host === 'codex')
    return {
      hooks: {
        UserPromptSubmit: [
          {
            hooks: [
              {
                type: 'command',
                command,
                command_windows: `node ${quotePowerShell(hookPath)}`,
                timeout: 5,
              },
            ],
          },
        ],
      },
    }

  return {
    version: 1,
    hooks: {
      postToolUse: [{ command }],
    },
  }
}

/** @param {string[]} args @param {string} moduleUrl @param {string} platform */
export function renderSessionCheckConfigOutput(
  args,
  moduleUrl = import.meta.url,
  platform = process.platform,
) {
  const host = parseSessionCheckConfigArgs(args)
  const hookPath = fileURLToPath(new URL('./session-check-hook.mjs', moduleUrl))
  if (!existsSync(hookPath)) throw new Error('The packaged session-check-hook.mjs is missing.')
  return `${JSON.stringify(renderSessionCheckConfig({ host, hookPath, platform }), null, 2)}\n`
}

if (
  typeof process.argv[1] === 'string' &&
  pathToFileURL(realpathSync(resolve(process.argv[1]))).href ===
    pathToFileURL(realpathSync(resolve(import.meta.filename))).href
) {
  try {
    process.stdout.write(renderSessionCheckConfigOutput(process.argv.slice(2)))
  } catch (error) {
    process.stderr.write(
      `${error instanceof Error ? error.message : 'Unable to render hook configuration.'}\n`,
    )
    process.exitCode = 2
  }
}
