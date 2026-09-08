import { Suspense, useState, useSyncExternalStore } from "react"

import { PopoverView, SessionPaneLoading } from "./PopoverView"
import { PopoverSession, sessionPaneState } from "./popover/PopoverSession"
import { SessionPane, type SessionSubject } from "./popover/SessionPane"

/**
 * The debug-only session window: the activity list beside one session's
 * detail, in an ordinary resizable window.
 *
 * It exists so the detail can be seen and tuned at any width while the app
 * runs. `src-tauri/src/session_window.rs` opens it from the tray in debug
 * builds only. It is not the larger view the product is growing toward and
 * does not try to look like it: no sidebar, no title bar of its own.
 *
 * One `PopoverSession` serves both columns. The list renders through
 * `PopoverView` with `detail="aside"`, so opening a row pushes the subject
 * onto the shared stack and this view renders the detail beside the list.
 */
export function SessionWindowView() {
  const [session] = useState(() => new PopoverSession({ shell: "window" }))
  const state = useSyncExternalStore(
    session.subscribe,
    session.getSnapshot,
    session.getSnapshot,
  )
  const pane = sessionPaneState(state)
  const traverse = (subject: SessionSubject | undefined) =>
    subject ? () => session.replaceTop(subject) : undefined

  return (
    <div className="flex h-full bg-surface-window text-label">
      {/* The same width the popover window has (`WIDTH` in popover.rs), so
          the list renders exactly as it does there. */}
      <aside className="h-full w-[380px] shrink-0 border-r border-separator">
        <PopoverView session={session} detail="aside" />
      </aside>
      <main className="min-h-0 min-w-0 flex-1">
        {pane ? (
          <Suspense fallback={<SessionPaneLoading />}>
            <SessionPane
              subject={pane.subject}
              payload={pane.payload}
              loading={pane.loading}
              refreshing={pane.refreshing}
              error={pane.error}
              onBack={session.goBack}
              onPrev={traverse(pane.prev)}
              onNext={traverse(pane.next)}
              onOpenSession={session.openSession}
              onDeleted={session.sessionDeleted}
            />
          </Suspense>
        ) : (
          <p className="type-body flex h-full items-center justify-center text-label-tertiary">
            Select a session
          </p>
        )}
      </main>
    </div>
  )
}
