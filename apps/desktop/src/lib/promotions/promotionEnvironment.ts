import {
  getMainWindowVisible,
  onMainWindowVisibilityChanged,
  getPromotionReadiness,
  onOnboardingChanged,
} from "../ipc"
import type { PromotionEnvironment } from "./PromotionSession"
import { PROMOTION_LAUNCHER_OPENED_KEY, PROMOTION_SEEN_KEY } from "./PromotionSession"
let manualReady = false
let revision = 0
async function refreshReadiness() {
  const request = ++revision
  const readiness = await getPromotionReadiness().catch(() => ({
    manual: false,
    automatic: false,
  }))
  if (request !== revision) return undefined
  manualReady = readiness.manual
  return readiness.automatic
}
export const promotionEnvironment: PromotionEnvironment = {
  isManualReady: () => manualReady,
  loadReady: async () => (await refreshReadiness()) ?? false,
  onReady: async (listener) => {
    let active = true
    let refresh = 0
    const sync = () => {
      const request = ++refresh
      void refreshReadiness().then((ready) => {
        if (active && request === refresh && ready !== undefined) listener(ready)
      })
    }
    const changed = () => {
      manualReady = false
      listener(false)
      sync()
    }
    const stop = await onOnboardingChanged(changed).catch(() => () => {})
    window.addEventListener("focus", sync)
    sync()
    return () => {
      active = false
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
