import { useState, useSyncExternalStore } from "react"
import { DesktopOnboardingSession } from "../lib/DesktopOnboardingSession"
import { isMacOS } from "../lib/platform"
import { OnboardingView } from "../onboarding/OnboardingView"

export function DesktopOnboardingEntry() {
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
  return (
    <div className="h-full">
      {snapshot.state === null ? (
        <main
          className="flex h-full flex-col items-center justify-center gap-4 bg-surface-window p-6 text-label"
          aria-label="Introduction"
        >
          <p className="type-body" role={snapshot.error ? "alert" : "status"}>
            {snapshot.error ?? "Loading your introduction…"}
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
