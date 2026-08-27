import { useSyncExternalStore } from "react"

/**
 * Each window publishes its surface name for shared styles. The resident shell
 * uses a URL fragment to select the popover, nudge, or overlay view. Standalone
 * windows use dedicated entries and pass their route directly.
 */
export type Route = "popover" | "settings" | "nudge" | "onboarding" | "overlay" | "hud-detail"

type ShellRoute = Exclude<Route, "settings" | "onboarding">

/** Fragment the nudge crate opens the notification window with. */
export const NUDGE_FRAGMENT = "#/nudge"

/** Fragment the HUD crate opens for the floating usage window. */
export const OVERLAY_FRAGMENT = "#/overlay"

// A Map, not a plain object: the fragment is outside input, and an object
// index would resolve inherited names ("constructor") to functions rather
// than falling back to the popover.
const ROUTES = new Map<string, ShellRoute>([
  ["nudge", "nudge"],
  ["overlay", "overlay"],
  ["hud-detail", "hud-detail"],
])

export function routeFromHash(hash: string): ShellRoute {
  return ROUTES.get(hash.replace(/^#\/?/, "")) ?? "popover"
}

/**
 * Publishes the route to `<html data-route>` so CSS can branch on the window.
 * Call once, before the first render: a window keeps its route for its whole
 * lifetime, so this attribute never has to change.
 */
export function applyRouteAttribute(
  root: HTMLElement = document.documentElement,
  route: Route = routeFromHash(window.location.hash),
): void {
  root.setAttribute("data-route", route)
}

function subscribe(onChange: () => void): () => void {
  window.addEventListener("hashchange", onChange)
  return () => window.removeEventListener("hashchange", onChange)
}

function currentRoute(): ShellRoute {
  return routeFromHash(window.location.hash)
}

/** Reads the active route and re-renders if the fragment ever changes. */
export function useRoute(): ShellRoute {
  return useSyncExternalStore(subscribe, currentRoute, () => "popover" as const)
}
