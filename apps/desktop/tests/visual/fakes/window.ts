declare global {
  interface Window {
    __ANTIBURN_VISUAL_WINDOW_ACTIONS__?: string[]
  }
}

let maximized = false
const resizeListeners = new Set<() => void>()

function recordAction(action: string) {
  window.__ANTIBURN_VISUAL_WINDOW_ACTIONS__ ??= []
  window.__ANTIBURN_VISUAL_WINDOW_ACTIONS__.push(action)
}

export async function currentMonitor() {
  return { scaleFactor: 1 }
}

export function getCurrentWindow() {
  return {
    async close() {
      recordAction("close")
    },
    async minimize() {
      recordAction("minimize")
    },
    async isMaximized() {
      return maximized
    },
    async toggleMaximize() {
      recordAction("toggleMaximize")
      maximized = !maximized
      resizeListeners.forEach((listener) => listener())
    },
    async onResized(listener: () => void) {
      resizeListeners.add(listener)
      return () => {
        resizeListeners.delete(listener)
      }
    },
    async startDragging() {
      recordAction("startDragging")
    },
    async startResizeDragging(direction: string) {
      recordAction(`resize:${direction}`)
    },
    async isVisible() {
      return true
    },
    async outerPosition() {
      return { x: 0, y: 0 }
    },
    async setPosition() {},
  }
}
