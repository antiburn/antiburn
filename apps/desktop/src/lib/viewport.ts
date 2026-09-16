import { useSyncExternalStore } from "react"

function subscribe(listener: () => void): () => void {
  window.addEventListener("resize", listener)
  window.addEventListener("antiburn:interface-scale-changed", listener)
  return () => {
    window.removeEventListener("resize", listener)
    window.removeEventListener("antiburn:interface-scale-changed", listener)
  }
}

function width(): number {
  return window.innerWidth
}

/** React observes the webview's CSS viewport, independently of display resolution. */
export function useViewportWidth(): number {
  return useSyncExternalStore(subscribe, width, () => 1100)
}
