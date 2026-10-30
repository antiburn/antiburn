import { getMainWindowVisible, onMainWindowVisibilityChanged } from "../ipc"
import type { PromotionEnvironment } from "./PromotionSession"
import { PROMOTION_LAUNCHER_OPENED_KEY, PROMOTION_SEEN_KEY } from "./PromotionSession"
import { PromotionTiming, PROMOTION_FTUE_KEY } from "./PromotionTiming"
import {
  overviewProgress,
  subscribeOverviewProgress,
} from "../../views/main-window/overview/overviewProgressStore"

const timing = new PromotionTiming({
  read: () => localStorage.getItem(PROMOTION_FTUE_KEY),
  write: (value) => localStorage.setItem(PROMOTION_FTUE_KEY, value),
  now: () => Date.now(),
})
const ready = () => timing.ready(overviewProgress())

export const promotionEnvironment: PromotionEnvironment = {
  isManualReady: () => {
    const progress = overviewProgress()
    return (
      progress.mode === "steady" || (progress.mode === "firstRun" && progress.flow === "done")
    )
  },
  loadReady: async () => ready(),
  onReady: async (listener) => {
    const sync = () => listener(ready())
    const stop = subscribeOverviewProgress(sync)
    window.addEventListener("focus", sync)
    sync()
    return () => {
      stop()
      window.removeEventListener("focus", sync)
    }
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
