import { Component, type ErrorInfo, type ReactNode } from "react"

import { classifyMainWindowFailure } from "../lib/bootstrapDiagnostics"
import {
  reportMainWindowRenderFailure,
  reportMainWindowRenderStatus,
  requestMainWindowRecovery,
} from "../lib/ipc"
import { markFallbackCommitted, markHealthyCommitted } from "../lib/rendererHealth"

interface Props {
  children: ReactNode
}

interface State {
  failed: boolean
  retryPending: boolean
  retryFailed: boolean
}

function rendererGeneration(): number | null {
  const value = window.__ANTIBURN_WINDOW_GENERATION__
  return typeof value === "number" && Number.isSafeInteger(value) ? value : null
}

/** Keeps the main native window usable when the application tree cannot render. */
export class MainWindowErrorBoundary extends Component<Props, State> {
  state: State = { failed: false, retryPending: false, retryFailed: false }
  private healthyCandidate: HTMLSpanElement | null = null
  private healthyReported = false

  static getDerivedStateFromError(): Partial<State> {
    return { failed: true }
  }

  componentDidCatch(error: unknown, info: ErrorInfo): void {
    this.healthyCandidate = null
    markFallbackCommitted()
    const generation = rendererGeneration()
    if (generation !== null) {
      void reportMainWindowRenderStatus(generation, "fallback").catch(() => undefined)
      void reportMainWindowRenderFailure(
        generation,
        classifyMainWindowFailure("render_fallback", error),
      ).catch(() => undefined)
    }
    console.error("The main window application tree failed to render.", error, info)
  }

  private markHealthy = (node: HTMLSpanElement | null): void => {
    this.healthyCandidate = node
    if (!node || this.healthyReported) return
    queueMicrotask(() => {
      if (this.healthyCandidate !== node || this.state.failed || this.healthyReported) return
      this.healthyReported = true
      if (!markHealthyCommitted()) return
      const generation = rendererGeneration()
      if (generation !== null) {
        void reportMainWindowRenderStatus(generation, "healthy").catch(() => undefined)
      }
    })
  }

  private retry = (): void => {
    if (this.state.retryPending) return
    const generation = rendererGeneration()
    if (generation === null) {
      this.setState({ retryFailed: true })
      return
    }
    this.setState({ retryPending: true, retryFailed: false })
    void requestMainWindowRecovery(generation).catch(() => {
      this.setState({ retryPending: false, retryFailed: true })
    })
  }

  render(): ReactNode {
    if (this.state.failed) {
      return (
        <main className="flex min-h-screen items-center justify-center bg-surface-window p-6 text-label">
          <section className="flex max-w-md flex-col items-start gap-3 rounded-control border border-separator bg-surface-card p-5">
            <h1 className="type-title-2">The main window could not load</h1>
            <p className="type-body text-label-secondary">
              Your local data is unchanged. Reload this window to try again.
            </p>
            <button
              type="button"
              className="ui-push-button"
              disabled={this.state.retryPending}
              onClick={this.retry}
            >
              {this.state.retryPending ? "Reloading…" : "Reload"}
            </button>
            {this.state.retryFailed ? (
              <p className="type-footnote text-system-red-text">
                Reload could not start. Try again.
              </p>
            ) : null}
          </section>
        </main>
      )
    }
    return (
      <>
        <span ref={this.markHealthy} hidden aria-hidden data-main-window-healthy-marker />
        {this.props.children}
      </>
    )
  }
}
