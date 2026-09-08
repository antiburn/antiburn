import { mountWindow } from "./bootstrap"
import { applyTheme } from "./lib/appearance"
import { getSettings, mainWindowReady, onSettingsChanged } from "./lib/ipc"
import { MainWindowView } from "./views/MainWindowView"

/** Synchronizes the retained renderer with the stored theme preference. */
export class MainWindowThemeSession {
  private active = true
  private started = false
  private settingsVersion = 0
  private unlisten: (() => void) | null = null

  async start(): Promise<void> {
    if (!this.active || this.started) return
    this.started = true

    try {
      const unlisten = await onSettingsChanged((settings) => {
        if (!this.active) return
        this.settingsVersion += 1
        applyTheme(settings.theme)
      })
      if (!this.active) {
        unlisten()
        return
      }
      this.unlisten = unlisten
    } catch {
      // The stored snapshot below still supplies an initial theme.
    }

    const settings = await getSettings().catch(() => null)
    if (this.active && this.settingsVersion === 0 && settings) applyTheme(settings.theme)
  }

  dispose(): void {
    if (!this.active) return
    this.active = false
    this.unlisten?.()
    this.unlisten = null
  }
}

export async function mountMainWindow(): Promise<void> {
  const themeSession = new MainWindowThemeSession()
  const dispose = () => themeSession.dispose()
  window.addEventListener("pagehide", dispose, { once: true })
  if (import.meta.hot) {
    import.meta.hot.dispose(() => {
      window.removeEventListener("pagehide", dispose)
      dispose()
    })
  }

  await themeSession.start()

  mountWindow(<MainWindowView />, "main", mainWindowReady)
}
