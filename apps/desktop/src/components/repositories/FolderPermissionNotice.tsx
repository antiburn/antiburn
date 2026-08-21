// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { FolderLock } from "lucide-react"

import { PushButton } from "../ui/PushButton"
import type { FlowPhase } from "../../lib/useFolderPermissionFlow"
import type { DeferredPermissionDir } from "../../lib/types/repository"

/**
 * The explanation that comes *before* the system's consent dialog.
 *
 * This exists because the dialog on its own is not an explanation. It names a
 * folder and an app and offers two buttons, and a reader who did not ask for it
 * has no way to tell whether the request is reasonable. So antiburn says what
 * it wants and why first, and the dialog only ever appears in response to a
 * button on this notice.
 *
 * It reports rather than nags: it is not dismissible because it is not an
 * interruption — it sits in a list that is visibly missing entries, and it
 * explains the gap.
 */
export function FolderPermissionNotice({
  deferred,
  phase,
  current,
  position,
  total,
  recordedDenials,
  onRequest,
  onOpenSettings,
  onRecheck,
  onCopyDiagnostics,
  rechecking = false,
}: {
  deferred: DeferredPermissionDir[]
  phase: FlowPhase
  current: string | null
  position: number
  total: number
  recordedDenials: string[]
  onRequest: () => void
  onOpenSettings: () => void
  /** Look for access granted outside antiburn, in system settings. */
  onRecheck: () => void
  onCopyDiagnostics: () => void
  rechecking?: boolean
}) {
  if (deferred.length === 0) return null

  // Every folder left is one the system will not re-prompt for, so the button
  // that asks would do nothing. Send the reader somewhere that works instead.
  const allStuck =
    deferred.length > 0 && deferred.every((entry) => recordedDenials.includes(entry.dir))
  const busy = phase === "asking" || phase === "settling"

  return (
    <div role="status" className="rounded-lg border border-separator bg-surface-card px-4 py-3">
      <div className="flex items-start gap-3">
        <FolderLock aria-hidden className="mt-0.5 size-4 shrink-0 text-label-secondary" />
        <div className="min-w-0 flex-1 space-y-2">
          <p className="type-footnote text-label">
            {allStuck ? headlineForStuck(deferred) : headlineForPending(deferred)}
          </p>
          <p className="type-caption text-label-secondary">
            {allStuck
              ? "macOS remembers an earlier “Don’t Allow” for these folders and will not ask again. Turning antiburn on in System Settings is the way back."
              : "antiburn looks for the git repositories your coding agents worked in. It reads repository names and locations only, and nothing leaves this Mac."}
          </p>

          {busy && current !== null ? (
            <p className="type-caption text-label-secondary" aria-live="polite">
              {phase === "settling"
                ? `Reading ${current}…`
                : `Waiting for macOS about ${current}. If no dialog appears, check your other windows and displays.`}
              {total > 1 ? ` (${position} of ${total})` : ""}
            </p>
          ) : null}

          <div className="flex flex-wrap items-center gap-2 pt-0.5">
            {allStuck ? (
              <>
                <PushButton onClick={onOpenSettings} variant="primary">
                  Open System Settings
                </PushButton>
                {/* Turning antiburn on in System Settings changes nothing here
                    until something looks again, and nothing may look for an
                    hour. This is that "look again". */}
                <PushButton onClick={onRecheck} disabled={rechecking}>
                  {rechecking ? "Checking…" : "Check again"}
                </PushButton>
                <PushButton onClick={onCopyDiagnostics}>Copy diagnostics</PushButton>
              </>
            ) : (
              <>
                <PushButton onClick={onRequest} variant="primary" disabled={busy}>
                  {busy ? "Waiting for macOS…" : "Grant access"}
                </PushButton>
                <PushButton onClick={onOpenSettings}>Open System Settings</PushButton>
                <PushButton onClick={onRecheck} disabled={busy || rechecking}>
                  {rechecking ? "Checking…" : "Check again"}
                </PushButton>
              </>
            )}
          </div>
        </div>
      </div>
    </div>
  )
}

/** "antiburn can't read Documents and Desktop yet." */
function headlineForPending(deferred: DeferredPermissionDir[]): string {
  return `antiburn can’t read ${joinFolders(deferred.map((entry) => entry.dir))} yet.`
}

function headlineForStuck(deferred: DeferredPermissionDir[]): string {
  return `antiburn is blocked from ${joinFolders(deferred.map((entry) => entry.dir))}.`
}

/** `a`, `a and b`, `a, b and c` — the shape a sentence wants. */
export function joinFolders(names: string[]): string {
  if (names.length <= 1) return names[0] ?? ""
  if (names.length === 2) return `${names[0]} and ${names[1]}`
  return `${names.slice(0, -1).join(", ")} and ${names[names.length - 1]}`
}
