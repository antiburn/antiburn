export async function currentMonitor() {
  return { scaleFactor: 1 }
}

export function getCurrentWindow() {
  return {
    async close() {},
    async isVisible() {
      return true
    },
    async outerPosition() {
      return { x: 0, y: 0 }
    },
    async setPosition() {},
  }
}
