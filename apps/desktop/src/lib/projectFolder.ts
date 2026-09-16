import { writeClipboardText } from "./clipboard"
import { noteInteraction, openProjectFolder } from "./ipc"

/** Report only the outcome of an explicit local folder action. */
export async function performProjectFolderAction(path: string, action: "open" | "copy") {
  try {
    await (action === "open" ? openProjectFolder(path) : writeClipboardText(path))
    noteInteraction({ kind: "projectFolderAction", action, outcome: "succeeded" })
  } catch (error) {
    noteInteraction({ kind: "projectFolderAction", action, outcome: "failed" })
    throw error
  }
}
