import { useState, useSyncExternalStore } from "react"
import { DesktopOnboardingSession } from "../lib/DesktopOnboardingSession"
import { createExternalStore } from "../lib/externalStore"
import { appInfo } from "../lib/ipc"
import { isMacOS } from "../lib/platform"
import { OnboardingView } from "../onboarding/OnboardingView"

const CLOUD_CAPABILITY_TIMEOUT_MS = 5_000

/** Use the informational screen if the build lookup cannot finish in time. */
async function readCloudCapability(): Promise<boolean> {
  let timeout: ReturnType<typeof setTimeout> | undefined
  try {
    return await Promise.race([
      appInfo().then(
        (info) => info?.cloudEnabled === true,
        () => false,
      ),
      new Promise<boolean>((resolve) => {
        timeout = setTimeout(() => resolve(false), CLOUD_CAPABILITY_TIMEOUT_MS)
      }),
    ])
  } finally {
    clearTimeout(timeout)
  }
}

/** Read the desktop Cloud build capability once for this introduction. */
export function DesktopOnboardingEntry() {
  const [cloudCapability] = useState(() => {
    let lookup: Promise<boolean> | undefined
    return createExternalStore<boolean | null>({
      initial: null,
      load: () => (lookup ??= readCloudCapability()),
    })
  })
  const cloudEnabled = useSyncExternalStore(
    cloudCapability.subscribe,
    cloudCapability.getSnapshot,
  )
  const [session] = useState(() => new DesktopOnboardingSession())
  const snapshot = useSyncExternalStore(
    session.subscribe,
    session.getSnapshot,
    session.getSnapshot,
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
  const checkingCloud =
    cloudEnabled === null &&
    (snapshot.state === "cloud" || snapshot.state === "cloudUnavailable")
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
