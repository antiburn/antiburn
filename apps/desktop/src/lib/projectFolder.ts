import { writeClipboardText } from "./clipboard"
import { noteInteraction, openProjectFolder, type ProjectFolderTarget } from "./ipc"

/** Report only the outcome of an explicit local folder action. */
export async function performProjectFolderAction(
  path: string,
  action: "open" | "copy",
  target: ProjectFolderTarget,
) {
  try {
    await (action === "open" ? openProjectFolder(target) : writeClipboardText(path))
    noteInteraction({ kind: "projectFolderAction", action, outcome: "succeeded" })
  } catch (error) {
    noteInteraction({ kind: "projectFolderAction", action, outcome: "failed" })
    throw error
  }
}
