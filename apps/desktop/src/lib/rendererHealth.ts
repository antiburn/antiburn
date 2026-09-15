export type RendererHealth = "mounting" | "healthy" | "fallback"

type SettledListener = (health: Exclude<RendererHealth, "mounting">) => void

let health: RendererHealth = "mounting"
let settledListeners = new Set<SettledListener>()

function settle(next: Exclude<RendererHealth, "mounting">): boolean {
  if (health === "fallback" || health === next) return false
  health = next
  const listeners = settledListeners
  settledListeners = new Set()
  for (const listener of listeners) listener(next)
  return true
}

/** Record the first healthy main-tree commit. */
export function markHealthyCommitted(): boolean {
  return settle("healthy")
}

/** Record a committed fallback. A fallback can replace a healthy tree. */
export function markFallbackCommitted(): boolean {
  return settle("fallback")
}

/** Read the committed renderer state without inferring native presentation. */
export function rendererHealth(): RendererHealth {
  return health
}

/** Run once after this renderer commits either its application or fallback tree. */
export function onRendererHealthSettled(listener: SettledListener): () => void {
  if (health !== "mounting") {
    listener(health)
    return () => undefined
  }
  settledListeners.add(listener)
  return () => settledListeners.delete(listener)
}

/** Restore module state for isolated tests. */
export function resetRendererHealthForTest(): void {
  health = "mounting"
  settledListeners.clear()
}
