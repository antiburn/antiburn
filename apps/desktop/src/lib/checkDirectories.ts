import { invoke } from "@tauri-apps/api/core"

import { hasShell } from "./ipc"

export type CheckDirectory = {
  reference: string | null
  scope: "global" | "project"
  label: string
  path: string
  state: "ready" | "missing" | "paused" | "unavailable"
  fileCount: number | null
  message: string | null
}

export async function getCheckDirectories(): Promise<CheckDirectory[]> {
  if (!hasShell()) throw new Error("Check folders are available only in the desktop app.")
  return invoke<CheckDirectory[]>("list_installed_check_directories")
}

export async function openCheckDirectory(reference: string): Promise<void> {
  if (!hasShell()) throw new Error("Check folders are available only in the desktop app.")
  return invoke<void>("open_installed_check_directory", { directoryReference: reference })
}
