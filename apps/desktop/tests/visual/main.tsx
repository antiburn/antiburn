import { StrictMode, type ComponentType } from "react"
import { createRoot } from "react-dom/client"
import { installFocusModality } from "../../src/lib/focusModality"
import { setHudTokenMapEnabled } from "../../src/lib/overlayWindow"

import "../../src/styles.css"
import "./visual.css"

type Surface =
  "main" | "settings" | "onboarding" | "popover" | "preview" | "hud" | "hud-detail" | "nudge"

const SURFACES: readonly Surface[] = [
  "main",
  "settings",
  "onboarding",
  "popover",
  "preview",
  "hud",
  "hud-detail",
  "nudge",
]

function querySurface(): Surface {
  const value = new URLSearchParams(window.location.search).get("surface")
  return SURFACES.includes(value as Surface) ? (value as Surface) : "main"
}

function configureDocument(): void {
  const params = new URLSearchParams(window.location.search)
  setHudTokenMapEnabled(params.get("map") === "on")
  const scale = Number(params.get("scale")) || 100
  const theme = params.get("theme") === "dark" ? "dark" : "light"
  const requestedPlatform = params.get("platform")
  const platform =
    requestedPlatform === "windows" || requestedPlatform === "linux"
      ? requestedPlatform
      : "macos"
  const platformName = { macos: "macOS", windows: "Windows", linux: "Linux" }[platform]
  Object.defineProperty(navigator, "userAgentData", {
    configurable: true,
    value: { platform: platformName },
  })
  Object.defineProperty(navigator, "userAgent", {
    configurable: true,
    value: `Antiburn visual fixture (${platformName})`,
  })
  document.documentElement.dataset.theme = theme
  document.documentElement.dataset.platform = platform
  document.documentElement.dataset.route = querySurface()
  document.documentElement.style.setProperty("--interface-scale", String(scale / 100))
  const fixtureWindow = window as Window & { __ANTIBURN_INTERFACE_SCALE_PERCENT__?: number }
  fixtureWindow.__ANTIBURN_INTERFACE_SCALE_PERCENT__ = scale
  window.dispatchEvent(new Event("antiburn:interface-scale-changed"))
}

async function loadSurface(surface: Surface): Promise<ComponentType> {
  switch (surface) {
    case "main":
      return (await import("../../src/views/MainWindowView")).MainWindowView
    case "settings":
      return (await import("../../src/views/SettingsView")).SettingsView
    case "onboarding":
      return (await import("../../src/views/OnboardingView")).OnboardingView
    case "popover":
      return (await import("../../src/views/PopoverView")).PopoverView
    case "preview":
      return (await import("../../src/views/PopoverPeekView")).PopoverPeekView
    case "hud":
      return (await import("../../src/views/OverlayWindow")).OverlayWindow
    case "hud-detail":
      return (await import("../../src/views/overlay/HudDetailView")).HudDetailView
    case "nudge":
      return (await import("../../src/views/NudgeView")).NudgeView
  }
}

function FixtureChrome({ surface }: { surface: Surface }) {
  const params = new URLSearchParams(window.location.search)
  return (
    <nav className="fixture-chrome" aria-label="Visual fixture controls">
      <strong>Interface scale QA</strong>
      <span data-testid="fixture-surface">{surface}</span>
      <span data-testid="fixture-state">{params.get("state") ?? "populated"}</span>
      <span data-testid="fixture-scale">{params.get("scale") ?? "100"}%</span>
    </nav>
  )
}

configureDocument()
installFocusModality()
const surface = querySurface()
const View = await loadSurface(surface)

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <div className="fixture-page" data-testid="fixture-page">
      <FixtureChrome surface={surface} />
      <div className="fixture-viewport" data-testid="fixture-viewport">
        <div className="fixture-scale-root" data-testid="fixture-scale-root">
          <View />
        </div>
      </div>
    </div>
  </StrictMode>,
)
