import { ExternalLink } from "lucide-react"
import { useState, useSyncExternalStore } from "react"
import { PushButton } from "../../components/ui/PushButton"
import type { CheckDirectory } from "../../lib/checkDirectories"
import { detectPlatform } from "../../lib/platform"
import { CheckDirectorySession } from "./CheckDirectorySession"
import { CheckSettingsDisclosure } from "./CheckSettingsDisclosure"

export function CheckFilesSettings({
  control,
  targetRevision,
}: {
  control?: string | null | undefined
  targetRevision?: number | undefined
}) {
  const [session] = useState(() => new CheckDirectorySession())
  const state = useSyncExternalStore(
    session.subscribe,
    session.getSnapshot,
    session.getSnapshot,
  )
  const platform = detectPlatform()
  const directories = state.directories?.filter(
    (directory): directory is CheckDirectory & { fileCount: number } =>
      directory.state === "ready" &&
      directory.fileCount !== null &&
      Number.isSafeInteger(directory.fileCount) &&
      directory.fileCount > 0,
  )
  const openLabel =
    platform === "macos"
      ? "Open in Finder"
      : platform === "windows"
        ? "Open in File Explorer"
        : "Open in file manager"
  return (
    <div>
      <CheckSettingsDisclosure
        searchId="checkDirectories"
        control={control}
        targetRevision={targetRevision}
        summary={
          state.error ? (
            <span role="alert">{state.error}</span>
          ) : state.openErrors.size > 0 ? (
            <span role="alert">
              Could not open a check folder. Reopen Settings → Checks to try again.
            </span>
          ) : undefined
        }
      >
        <div className="space-y-3 type-footnote text-label-secondary">
          {!state.directories && state.loading && <p role="status">Loading check folders…</p>}
          {directories?.length === 0 && <p>No check files found.</p>}
          {directories?.map((directory) => {
            const reference = directory.reference
            return (
              <div key={`${directory.scope}:${directory.path}`} className="min-w-0 space-y-1">
                <div className="flex min-w-0 flex-wrap items-center justify-between gap-2">
                  <div className="min-w-0">
                    <p className="wrap-anywhere type-callout text-label">{directory.label}</p>
                    <p>
                      {directory.fileCount.toLocaleString()} check{" "}
                      {directory.fileCount === 1 ? "file" : "files"}
                    </p>
                  </div>
                  {reference && (
                    <PushButton
                      trailingIcon={ExternalLink}
                      ariaLabel={`${openLabel}: ${directory.label} (${directory.path})`}
                      disabled={state.opening.has(reference)}
                      onClick={() => void session.open(reference)}
                    >
                      {state.opening.has(reference) ? "Opening…" : openLabel}
                    </PushButton>
                  )}
                </div>
                <p className="wrap-anywhere">{directory.path}</p>
                {reference && state.openErrors.get(reference) && (
                  <p className="text-system-red-text">{state.openErrors.get(reference)}</p>
                )}
              </div>
            )
          })}
          {Boolean(directories?.length) && (
            <p>
              Add <code>.yml</code> or <code>.yaml</code> check files directly to these folders.
            </p>
          )}
        </div>
      </CheckSettingsDisclosure>
    </div>
  )
}
