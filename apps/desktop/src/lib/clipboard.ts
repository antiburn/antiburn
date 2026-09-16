import { writeText } from "@tauri-apps/plugin-clipboard-manager"

/** Write text through the shell clipboard capability. */
export function writeClipboardText(text: string): Promise<void> {
  return writeText(text)
}
