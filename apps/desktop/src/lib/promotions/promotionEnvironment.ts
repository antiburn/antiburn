import { getMainWindowVisible, onMainWindowVisibilityChanged } from "../ipc"
import type { PromotionEnvironment } from "./PromotionSession"
import { PROMOTION_LAUNCHER_OPENED_KEY, PROMOTION_SEEN_KEY } from "./PromotionSession"
export const promotionEnvironment: PromotionEnvironment = {
  isManualReady: () => true,
  loadReady: async () => true,
  onReady: async (listener) => {
    listener(true)
    return () => undefined
  },
  loadVisible: getMainWindowVisible,
  onVisible: onMainWindowVisibilityChanged,
  isForeground: () => document.visibilityState === "visible" && document.hasFocus(),
  onForeground(listener) {
    window.addEventListener("focus", listener)
    window.addEventListener("blur", listener)
    document.addEventListener("visibilitychange", listener)
    return () => {
      window.removeEventListener("focus", listener)
      window.removeEventListener("blur", listener)
      document.removeEventListener("visibilitychange", listener)
    }
  },
  readSeen: () => localStorage.getItem(PROMOTION_SEEN_KEY),
  writeSeen: (value) => localStorage.setItem(PROMOTION_SEEN_KEY, value),
  readLauncherOpened: () => localStorage.getItem(PROMOTION_LAUNCHER_OPENED_KEY),
  writeLauncherOpened: (value) => localStorage.setItem(PROMOTION_LAUNCHER_OPENED_KEY, value),
}
