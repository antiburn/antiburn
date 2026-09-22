import { Copy, Minus, Square, X } from "lucide-react"
import { useState, useSyncExternalStore } from "react"
import { Tooltip } from "../../components/presentation/Tooltip"
import { WindowChromeSession } from "./WindowChromeSession"

export function WindowControls() {
  const [session] = useState(() => new WindowChromeSession())
  const { maximized, error } = useSyncExternalStore(
    session.subscribe,
    session.getSnapshot,
    session.getSnapshot,
  )
  const maximizeLabel = maximized ? "Restore window" : "Maximize window"
  return (
    <div className="main-window-caption-controls" role="group" aria-label="Window controls">
      <Tooltip label="Minimize window">
        <button
          type="button"
          className="main-window-caption-button"
          aria-label="Minimize window"
          onClick={() => void session.perform("minimize")}
        >
          <Minus size={14} aria-hidden="true" />
        </button>
      </Tooltip>
      <Tooltip label={maximizeLabel}>
        <button
          type="button"
          className="main-window-caption-button"
          aria-label={maximizeLabel}
          onClick={() => void session.perform("toggleMaximize")}
        >
          {maximized ? (
            <Copy size={14} aria-hidden="true" />
          ) : (
            <Square size={14} aria-hidden="true" />
          )}
        </button>
      </Tooltip>
      <Tooltip label="Close window">
        <button
          type="button"
          className="main-window-caption-button main-window-caption-close"
          aria-label="Close window"
          onClick={() => void session.perform("close")}
        >
          <X size={16} aria-hidden="true" />
        </button>
      </Tooltip>
      {error && (
        <p
          role="alert"
          className="main-window-caption-error rounded-control bg-surface-overlay p-3 type-caption text-label"
        >
          {error}
        </p>
      )}
    </div>
  )
}
