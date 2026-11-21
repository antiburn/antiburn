import { mountWindow } from "./bootstrap"
import { applyTheme } from "./lib/appearance"
import { getSettings, onSettingsChanged } from "./lib/ipc"
import { DesktopOnboardingEntry } from "./views/DesktopOnboardingEntry"

let active = true
let revision = 0
let unlisten: (() => void) | undefined
const dispose = () => {
  active = false
  unlisten?.()
}
window.addEventListener("pagehide", dispose, { once: true })
if (import.meta.hot) import.meta.hot.dispose(dispose)
void onSettingsChanged((settings) => {
  if (!active) return
  revision += 1
  applyTheme(settings.theme)
})
  .then((stop) => {
    if (active) unlisten = stop
    else stop()
  })
  .catch(() => undefined)
void getSettings()
  .then((settings) => {
    if (active && revision === 0) applyTheme(settings.theme)
  })
  .catch(() => undefined)
if (window.__ANTIBURN_CLOUD_DEV__) {
  void import("./onboarding/CloudDev").then(({ CloudDev, revealCloudDev }) => {
    mountWindow(<CloudDev />, "onboarding", revealCloudDev)
  })
} else mountWindow(<DesktopOnboardingEntry />, "onboarding")
