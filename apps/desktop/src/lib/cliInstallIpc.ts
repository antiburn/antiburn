import { listen } from "./tauriEvents"
import { invoke, isTauri } from "@tauri-apps/api/core"

export interface CliRuntime {
  executable: string
  version: string
  sha256: string
  deliveryProtocols: string[]
}

export interface CliInstallState {
  runtime: CliRuntime | null
  diagnostic: string | null
}

export function getCliInstallState(): Promise<CliInstallState> {
  if (!isTauri()) return Promise.reject(new Error("CLI installation requires the desktop app."))
  return invoke("get_cli_install_state")
}

export function repairCliInstallation(): Promise<CliInstallState> {
  return invoke("repair_cli_installation")
}

export function testCliInstallation(): Promise<CliRuntime> {
  return invoke("test_cli_installation")
}

export async function onCliInstallState(onState: (state: CliInstallState) => void) {
  if (!isTauri()) return () => undefined
  return listen<CliInstallState>("cli-install:state", (event) => onState(event.payload))
}
