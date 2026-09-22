import { isTauri } from "@tauri-apps/api/core"
import { getCurrentWindow } from "@tauri-apps/api/window"
import { useState, useSyncExternalStore } from "react"
import { WindowChromeSession } from "./WindowChromeSession"

const directions = [
  "North",
  "NorthEast",
  "East",
  "SouthEast",
  "South",
  "SouthWest",
  "West",
  "NorthWest",
] as const

export function WindowResizeHandles() {
  const [session] = useState(() => new WindowChromeSession())
  const { maximized } = useSyncExternalStore(
    session.subscribe,
    session.getSnapshot,
    session.getSnapshot,
  )
  const [error, setError] = useState("")
  return (
    <>
      {!maximized &&
        directions.map((direction) => (
          <div
            key={direction}
            className="main-window-resize-edge"
            data-direction={direction}
            aria-hidden="true"
            onPointerDown={(event) => {
              if (event.button !== 0 || !isTauri()) return
              event.preventDefault()
              setError("")
              void getCurrentWindow()
                .startResizeDragging(direction)
                .catch(() => setError("Could not resize the window. Please try again."))
            }}
          />
        ))}
      {error && (
        <p
          role="alert"
          className="main-window-caption-error rounded-control bg-surface-overlay p-3 type-caption text-label"
        >
          {error}
        </p>
      )}
    </>
  )
}
