import { useState, useSyncExternalStore } from "react"
import { DesktopOnboardingSession } from "../lib/DesktopOnboardingSession"
import { createExternalStore } from "../lib/externalStore"
import { appInfo } from "../lib/ipc"
import { isMacOS } from "../lib/platform"
import { OnboardingView } from "../onboarding/OnboardingView"

const CLOUD_CAPABILITY_TIMEOUT_MS = 5_000

/** Read the build flag once. Limit the wait only while the Cloud step is open. */
function createCloudCapability() {
  let lookup: Promise<boolean> | undefined
  let finishLookup: ((enabled: boolean) => void) | undefined
  const store = createExternalStore<boolean | null>({
    initial: null,
    load: () =>
      (lookup ??= new Promise<boolean>((resolve) => {
        finishLookup = resolve
        void appInfo().then(
          (info) => resolve(info?.cloudEnabled === true),
          () => resolve(false),
        )
      }).finally(() => {
        finishLookup = undefined
      })),
  })
  return {
    getSnapshot: store.getSnapshot,
    subscribe: store.subscribe,
    subscribeAtCloud(listener: () => void) {
      const unsubscribe = store.subscribe(listener)
      if (!finishLookup) return unsubscribe
      const timeout = setTimeout(() => finishLookup?.(false), CLOUD_CAPABILITY_TIMEOUT_MS)
      void lookup?.then(() => clearTimeout(timeout))
      return () => {
        clearTimeout(timeout)
        unsubscribe()
      }
    },
  }
}

/** Read the desktop Cloud build capability once for this introduction. */
export function DesktopOnboardingEntry() {
  const [cloudCapability] = useState(createCloudCapability)
  const [session] = useState(() => new DesktopOnboardingSession())
  const snapshot = useSyncExternalStore(
    session.subscribe,
    session.getSnapshot,
    session.getSnapshot,
  )
  const atCloudStep = snapshot.state === "cloud" || snapshot.state === "cloudUnavailable"
  const cloudEnabled = useSyncExternalStore(
    atCloudStep ? cloudCapability.subscribeAtCloud : cloudCapability.subscribe,
    cloudCapability.getSnapshot,
  )
  if (snapshot.state === "completed")
    return (
      <main className="flex h-full flex-col items-center justify-center gap-4 bg-surface-window text-label">
        <p role={snapshot.error ? "alert" : "status"}>
          {snapshot.error ?? "Opening Sessions…"}
        </p>
        <button
          type="button"
          className="ui-push-button"
          disabled={snapshot.busy}
          onClick={() => void session.change("completed")}
        >
          Open Sessions
        </button>
      </main>
    )
  const checkingCloud = atCloudStep && cloudEnabled === null
  return (
    <div className="h-full">
      {snapshot.state === null || checkingCloud ? (
        <main
          className="flex h-full flex-col items-center justify-center gap-4 bg-surface-window p-6 text-label"
          aria-label="Introduction"
        >
          <p className="type-body" role={snapshot.error ? "alert" : "status"}>
            {snapshot.error ??
              (checkingCloud ? "Checking Cloud availability…" : "Loading your introduction…")}
          </p>
          {snapshot.error && (
            <button
              type="button"
              className="ui-push-button"
              onClick={() => void session.load()}
            >
              Try again
            </button>
          )}
        </main>
      ) : (
        <OnboardingView
          cloudEnabled={cloudEnabled === true}
          step={snapshot.state}
          onStepChange={(step) => void session.change(step)}
          onComplete={() => void session.change("completed")}
          busy={snapshot.busy}
          error={snapshot.error ?? undefined}
          hudSupported={isMacOS()}
          hudVariant={snapshot.hudVariant}
          visible={snapshot.visible}
        />
      )}
    </div>
  )
}
