import { invoke } from "@tauri-apps/api/core"

import { hasShell } from "./ipc"

/** Write the privacy-scoped support diagnostics to `destPath`. */
export async function exportDiagnostics(destPath: string): Promise<string | null> {
  if (!hasShell()) return null
  return invoke<string>("export_diagnostics", { destPath })
}
