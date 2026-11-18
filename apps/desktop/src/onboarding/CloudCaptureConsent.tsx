import {
  cloudUploadStatusUnknown,
  type CloudSession,
  type CloudViewSnapshot,
} from "./CloudSession"
import { personalCloudDestination } from "../lib/cloudDestination"
import {
  cloudProjectSelectionKey,
  type CloudProjectChoices,
  type CloudProjectGroup,
} from "../lib/cloud"
import { useId } from "react"

/** Show the exact account and destination. CloudSession starts confirmation
 * unchecked and clears it when authority or selected roots change.
 */
export function CloudCaptureConsent({
  session,
  value,
  projects,
}: {
  session: CloudSession
  value: CloudViewSnapshot
  /** Current grouped path members. The caller keeps row choices local until Start. */
  projects?: {
    groups: readonly CloudProjectGroup[]
    choices: CloudProjectChoices
    selectionKey?: string
    loading?: boolean
    error?: string | null
  }
}) {
  const destination = personalCloudDestination(value.cloud.destination ?? null)
  const selectedCount = projects
    ? projects.groups.filter((group) => projects.choices[group.id] ?? true).length
    : value.cloud.draft.folders.filter((folder) => folder.selected).length
  const selectionKey = projects
    ? (projects.selectionKey ?? cloudProjectSelectionKey(projects.groups, projects.choices))
    : null
  const confirmed =
    value.fullContentConsent && (!projects || value.fullContentConsentKey === selectionKey)
  const captureOnly = !projects && value.cloud.captureEnabled && !value.cloud.syncEnabled
  const contextId = useId()
  const hasScope = !!projects || value.cloud.syncEnabled
  return (
    <div className="flex flex-col gap-2 px-3 py-2 type-callout text-label">
      <p id={`${contextId}-account`}>
        Account: <strong>{value.cloud.identity?.login ?? "Not connected"}</strong>
        {" · "}Upload to:{" "}
        <strong>{value.cloud.destination?.name ?? "Not confirmed. Uploads are off."}</strong>
      </p>
      {!selectedCount && (
        <p className="type-footnote text-label-secondary">
          Select project folders before confirming uploads.
        </p>
      )}
      {value.cloud.destination?.kind === "team" && (
        <p role="status" className="type-footnote text-label-secondary">
          Team accounts are not supported here. Sign in with your personal Cloud account.
          Antiburn has not changed your selected account.
        </p>
      )}
      <label className="flex items-start gap-2">
        <input
          type="checkbox"
          aria-describedby={[
            `${contextId}-account`,
            `${contextId}-content`,
            ...(hasScope ? [`${contextId}-scope`] : []),
            `${contextId}-limits`,
          ].join(" ")}
          checked={confirmed}
          disabled={
            value.busy ||
            cloudUploadStatusUnknown(value.ongoing) ||
            projects?.loading ||
            !!projects?.error ||
            !value.cloud.identity ||
            value.cloud.status !== "signed_in" ||
            !destination ||
            !selectedCount ||
            (projects !== undefined && selectionKey === "[]") ||
            (projects !== undefined &&
              !projects.groups.some(
                (group) =>
                  group.captureSupported !== false && (projects.choices[group.id] ?? true),
              )) ||
            !!value.cloud.setupError
          }
          onChange={(event) =>
            session.setFullContentConsent(event.target.checked, selectionKey)
          }
        />
        <span>
          {captureOnly
            ? "I authorize a full-content capture request for the selected sources, account, and destination."
            : "Upload existing and new sessions in full from these folders and their worktrees to my personal Cloud account while uploads are on."}
        </span>
      </label>
      <p id={`${contextId}-content`} className="type-footnote text-label-secondary">
        Sessions can include messages, private thinking, tool activity and results, session
        details, and files saved inside an OpenCode session. They may contain secrets or private
        content.
      </p>
      {captureOnly && (
        <p className="type-footnote text-label-secondary">
          This action saves a capture request on this device. Upload is unavailable. Saving does
          not confirm that any source content was read or captured.
        </p>
      )}
      {hasScope && (
        <p id={`${contextId}-scope`} className="type-footnote text-label-secondary">
          A match uploads the whole session, including work in other folders. Subfolders are
          included. New worktrees outside these folders need approval.
        </p>
      )}
      {value.cloud.setupError && (
        <p className="type-footnote text-label-secondary" role="status">
          {value.cloud.setupError} Reload Cloud settings before choosing folders or confirming
          capture.
        </p>
      )}
      <p id={`${contextId}-limits`} className="type-footnote text-label-secondary">
        External attachments and linked-file downloads are excluded.{" "}
        {captureOnly
          ? "This build saves requests only; it does not read or upload content."
          : "Local analysis and Checks use separate settings."}
      </p>
    </div>
  )
}
