import { mountWindow } from "./bootstrap"
import { MainWindowErrorBoundary } from "./components/MainWindowErrorBoundary"
import { applyTheme } from "./lib/appearance"
import { installBootstrapDiagnostics, reportBootstrapFailure } from "./lib/bootstrapDiagnostics"
import { getSettings, mainWindowReady, onSettingsChanged } from "./lib/ipc"
import { installMainWindowHealthResponder } from "./lib/mainWindowHealth"
import { MainWindowView } from "./views/MainWindowView"

const RESPONDER_INSTALL_TIMEOUT = 1_000

/** Synchronizes the retained renderer with the stored theme preference. */
class MainWindowThemeSession {
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

async function guardedInstallResponder(): Promise<() => void> {
  let accepted = true
  let timeout: ReturnType<typeof setTimeout> | null = null
  const install = installMainWindowHealthResponder()
  const deadline = new Promise<never>((_, reject) => {
    timeout = setTimeout(
      () => reject(new Error("responder install timed out")),
      RESPONDER_INSTALL_TIMEOUT,
    )
  })
  try {
    const disposer = await Promise.race([install, deadline])
    if (timeout) clearTimeout(timeout)
    return disposer
  } catch (error) {
    accepted = false
    if (timeout) clearTimeout(timeout)
    void install
      .then((dispose) => {
        if (!accepted) dispose()
      })
      .catch(() => undefined)
    reportBootstrapFailure("responder_install_failed", error)
    console.error("The main-window health responder did not install.", error)
    return () => undefined
  }
}

export async function mountMainWindow(): Promise<void> {
  const disposeDiagnostics = installBootstrapDiagnostics()
  const themeSession = new MainWindowThemeSession()
  let disposeResponder: (() => void) | null = null
  let disposed = false
  const dispose = () => {
    if (disposed) return
    disposed = true
    disposeResponder?.()
    disposeDiagnostics()
    themeSession.dispose()
  }
  window.addEventListener("pagehide", dispose, { once: true })
  if (import.meta.hot) {
    import.meta.hot.dispose(() => {
      window.removeEventListener("pagehide", dispose)
      dispose()
    })
  }

  const responder = await guardedInstallResponder()
  if (disposed) {
    responder()
    return
  }
  disposeResponder = responder
  await themeSession.start()
  if (disposed) return

  mountWindow(
    <MainWindowErrorBoundary>
      <MainWindowView />
    </MainWindowErrorBoundary>,
    "main",
    mainWindowReady,
  )
}
