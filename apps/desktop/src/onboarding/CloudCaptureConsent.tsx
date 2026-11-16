import type { CloudSession, CloudViewSnapshot } from "./CloudSession"

export function CloudCaptureConsent({
  session,
  value,
}: {
  session: CloudSession
  value: CloudViewSnapshot
}) {
  return (
    <div className="flex flex-col gap-3 p-4 type-callout text-label">
      <p>
        Destination:{" "}
        <strong>{value.cloud.destination?.name ?? "Not verified. Capture is blocked."}</strong>
      </p>
      <label className="flex items-start gap-2">
        <input
          type="checkbox"
          checked={value.fullContentConsent}
          disabled={value.busy || !value.cloud.identity || !value.cloud.destination}
          onChange={(event) => session.setFullContentConsent(event.target.checked)}
        />
        <span>
          I agree to upload full native session content from the selected folders to this
          destination. This includes messages, private thinking, tool calls and results, and
          session metadata. It can contain secrets or private file text.
        </span>
      </label>
      <p className="type-footnote text-label-secondary">
        Attachments are separate. This build does not read or upload attachment files.
      </p>
    </div>
  )
}
