import type { CloudSession, CloudViewSnapshot } from "./CloudSession"
import { personalCloudDestination } from "../lib/cloudDestination"

export function CloudCaptureConsent({
  session,
  value,
}: {
  session: CloudSession
  value: CloudViewSnapshot
}) {
  const destination = personalCloudDestination(value.cloud.destination ?? null)
  const selected = value.cloud.draft.folders.filter((folder) => folder.selected)
  return (
    <div className="flex flex-col gap-3 p-4 type-callout text-label">
      <p>
        Account: <strong>{value.cloud.identity?.login ?? "Not connected"}</strong>
      </p>
      <p>
        Destination:{" "}
        <strong>{value.cloud.destination?.name ?? "Not verified. Capture is blocked."}</strong>
      </p>
      <p className="type-footnote text-label-secondary">
        {selected.length
          ? `${selected.length} ${selected.length === 1 ? "folder" : "folders"} selected for this account and destination.`
          : "Select folders before confirming full-content capture."}
      </p>
      {value.cloud.destination?.kind === "team" && (
        <p role="status" className="type-footnote text-label-secondary">
          This build supports personal workspaces only. The requested team has not been replaced
          with a personal destination.
        </p>
      )}
      <label className="flex items-start gap-2">
        <input
          type="checkbox"
          checked={value.fullContentConsent}
          disabled={
            value.busy ||
            !value.cloud.identity ||
            !destination ||
            !selected.length ||
            !!value.cloud.setupError
          }
          onChange={(event) => session.setFullContentConsent(event.target.checked)}
        />
        <span>
          I agree to upload full native session content from the selected folders to this
          destination. This includes messages, private thinking, tool calls and results, and
          session metadata. It can contain secrets or private file text.
        </span>
      </label>
      {value.cloud.setupError && (
        <p className="type-footnote text-label-secondary" role="status">
          {value.cloud.setupError} Reload Cloud settings before choosing folders or confirming
          capture.
        </p>
      )}
      <p className="type-footnote text-label-secondary">
        Attachments are separate. This build does not read or upload attachment files.
      </p>
    </div>
  )
}
