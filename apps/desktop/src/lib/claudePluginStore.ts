/**
 * The shell facts behind the Claude Code plugin offer, as one external store:
 * the plugin status, the live-usage meters that say whether Claude Code is
 * here, the local Claude Code session count, and the reader's dismissal.
 *
 * The store reads once when its first viewer arrives and again each time the
 * window becomes visible or focused, because a reader who copies the install
 * command installs it in Claude Code and comes back. There is no watcher and
 * no polling.
 *
 * The two analytics facts that are about exposure, not presses, are emitted
 * here at the store boundary rather than in render: `installed_detected`
 * once per run when the status read says installed, and `shown` once per
 * placement per run when that placement first has an offer to show.
 */

import { useState, useSyncExternalStore } from "react"

import {
  claudePluginOffer,
  readClaudePluginOfferDismissed,
  writeClaudePluginOfferDismissed,
  type ClaudePluginOfferState,
  type ClaudePluginStatusPayload,
} from "./claudePluginOffer"
import { createExternalStore, type ExternalStore } from "./externalStore"
import { getClaudePluginStatus, getLiveUsage, noteInteraction } from "./ipc"
import type { LiveUsageMeterPayload } from "./providerUsageIpc"

export type ClaudePluginPlacement = "sidebar" | "agents" | "usage_settings"

export interface ClaudePluginSnapshot {
  status: ClaudePluginStatusPayload | null
  meters: readonly LiveUsageMeterPayload[]
  claudeSessions: number
  dismissed: boolean
}

export interface ClaudePluginStore extends ExternalStore<ClaudePluginSnapshot> {
  /** Hide the sidebar card for good. */
  dismiss: () => void
}

/** A source for the local Claude Code session count. */
export interface ClaudeSessionsSource {
  read: () => number
  subscribe: (listener: () => void) => () => void
}

const EMPTY: ClaudePluginSnapshot = {
  status: null,
  meters: [],
  claudeSessions: 0,
  dismissed: false,
}

let installedReported = false
const shownPlacements = new Set<ClaudePluginPlacement>()

/** Test-only: forget which exposure facts this run has already sent. */
export function resetClaudePluginReports(): void {
  installedReported = false
  shownPlacements.clear()
}

export function createClaudePluginStore({
  reportInstalled,
  sessions,
}: {
  /**
   * Only the main window reports `installed_detected`, so the Settings window
   * does not count the same install twice.
   */
  reportInstalled: boolean
  sessions?: ClaudeSessionsSource
}): ClaudePluginStore {
  async function loadFacts(): Promise<ClaudePluginSnapshot> {
    const [status, live] = await Promise.all([
      getClaudePluginStatus().catch(() => null),
      getLiveUsage().catch(() => null),
    ])
    if (reportInstalled && status?.state === "installed" && !installedReported) {
      installedReported = true
      noteInteraction({
        kind: "claudePluginOffer",
        action: "installed_detected",
        placement: "startup",
      })
    }
    return {
      status,
      meters: live?.meters ?? [],
      claudeSessions: sessions?.read() ?? 0,
      dismissed: readClaudePluginOfferDismissed(),
    }
  }
  const store = createExternalStore<ClaudePluginSnapshot>({
    initial: EMPTY,
    load: loadFacts,
    // A placement that comes back later starts from a fresh read, not from
    // the facts its last viewer left behind.
    resetOnStop: true,
    subscribe: async (set) => {
      function reload(): void {
        if (document.hidden) return
        void loadFacts().then(set)
      }
      document.addEventListener("visibilitychange", reload)
      window.addEventListener("focus", reload)
      const stopSessions = sessions?.subscribe(() =>
        set({ ...store.getSnapshot(), claudeSessions: sessions.read() }),
      )
      return () => {
        document.removeEventListener("visibilitychange", reload)
        window.removeEventListener("focus", reload)
        stopSessions?.()
      }
    },
  })
  return {
    ...store,
    dismiss() {
      writeClaudePluginOfferDismissed()
      noteInteraction({ kind: "claudePluginOffer", action: "dismissed", placement: "sidebar" })
      store.set({ ...store.getSnapshot(), dismissed: true })
    },
  }
}

/** The offer state one placement shows. Status placements ignore the dismissal. */
export function offerForPlacement(
  snapshot: ClaudePluginSnapshot,
  placement: ClaudePluginPlacement,
): ClaudePluginOfferState {
  return claudePluginOffer({
    ...snapshot,
    dismissed: placement === "sidebar" && snapshot.dismissed,
  })
}

function noteShown(placement: ClaudePluginPlacement): void {
  if (shownPlacements.has(placement)) return
  shownPlacements.add(placement)
  noteInteraction({ kind: "claudePluginOffer", action: "shown", placement })
}

/**
 * Subscribe a placement to the store. Sends `shown` the first time this run
 * that the placement has an offer to show, at the moment the snapshot says so.
 */
function subscribePlacement(
  store: ClaudePluginStore,
  placement: ClaudePluginPlacement,
): (listener: () => void) => () => void {
  return (listener) => {
    function sync(): void {
      if (offerForPlacement(store.getSnapshot(), placement) === "offer") noteShown(placement)
      listener()
    }
    const stop = store.subscribe(sync)
    sync()
    return stop
  }
}

function subscribeInactive(): () => void {
  return () => undefined
}

/** The store snapshot for one placement, plus the offer state it shows. */
export function useClaudePlugin(
  store: ClaudePluginStore,
  placement: ClaudePluginPlacement,
  active = true,
): { snapshot: ClaudePluginSnapshot; offer: ClaudePluginOfferState } {
  const [subscribe] = useState(() => subscribePlacement(store, placement))
  const snapshot = useSyncExternalStore(
    active ? subscribe : subscribeInactive,
    store.getSnapshot,
    store.getSnapshot,
  )
  return { snapshot, offer: offerForPlacement(snapshot, placement) }
}
