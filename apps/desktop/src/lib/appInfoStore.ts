/**
 * App info for views outside the Settings window, such as the Agents and
 * Sessions step settings: the app version (remote-host downloads) and the
 * local index size (indexed sessions, database bytes).
 *
 * A single module-level store, built the same way as `scanStatusStore`: one
 * read on the first subscriber, refreshed whenever the session index
 * changes.
 */
import { createExternalStore } from "./externalStore"
import { appInfo, onSessionIndexChanged, type AppInfo } from "./ipc"

export const appInfoStore = createExternalStore<AppInfo | null>({
  initial: null,
  load: () => appInfo().catch(() => null),
  subscribe: (set) =>
    onSessionIndexChanged(() => {
      void appInfo()
        .then(set)
        .catch(() => undefined)
    }),
})
