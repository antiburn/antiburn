import { isAllowedPromotionUrl } from "../../promotions/links"
import { invoke, isTauri } from "@tauri-apps/api/core"
import { openSettingsWindow } from "../ipc"
import type { PromotionAction } from "../../promotions/types"

/** Open a validated destination after a reader activates its link. */
export async function runPromotionAction(
  action: PromotionAction,
  navigate: (screen: "announcement" | "setup") => void,
): Promise<void> {
  switch (action.type) {
    case "screen":
      navigate(action.target)
      return
    case "app":
      await openSettingsWindow(action.pane)
      return
    case "external": {
      const url = new URL(action.url)
      if (!isAllowedPromotionUrl(url)) {
        throw new Error("Unsupported promotion link")
      }
      if (isTauri()) await invoke("plugin:opener|open_url", { url: url.href })
      else {
        const opened = window.open(url.href, "_blank")
        if (!opened) throw new Error("Could not open promotion link")
        opened.opener = null
      }
    }
  }
}
