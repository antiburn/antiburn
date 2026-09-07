/**
 * Sandbox replacement for `@tauri-apps/api/core`.
 *
 * Vite aliases the package to this module in `--mode sandbox`. Each command
 * resolves from the fixture scenario in `commands.ts`. `isTauri` reports true
 * so that `hasShell()` lets every IPC wrapper run.
 */

import { runCommand } from "./commands"

export function isTauri(): boolean {
  return true
}

export async function invoke<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  return runCommand(command, args) as T
}
