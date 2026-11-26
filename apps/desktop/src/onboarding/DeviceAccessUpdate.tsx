import type { CloudAccessUpdate, CloudSnapshot } from "../lib/cloud"

type Props = {
  value: CloudSnapshot
  busy: boolean
  update: (action: CloudAccessUpdate) => Promise<boolean>
  cancel: () => Promise<boolean>
  onboarding?: boolean
}

/** Request more access and let the user cancel a pending update even without a session. */
export function DeviceAccessUpdate({ value, busy, update, cancel, onboarding }: Props) {
  const row = onboarding
    ? "flex flex-wrap items-center gap-2"
    : "flex flex-wrap items-center gap-2 border-t border-separator px-4 py-3"
  const button = onboarding ? "onboarding-secondary" : "ui-push-button"
  const text = onboarding
    ? "type-callout text-onboarding-muted"
    : "type-footnote text-label-secondary"
  if (value.accessUpdate === "waiting") {
    return (
      <div className={row}>
        <p className={text} role="status">
          Review device access in your browser.
          {value.identity && " Your current connection stays active."}
        </p>
        <button type="button" className={button} disabled={busy} onClick={() => void cancel()}>
          Cancel access update
        </button>
      </div>
    )
  }
  const options = [
    {
      action: "session_reads",
      show: !onboarding && value.sessionReadUpdateAvailable,
      description: "Approve this device to search and read Cloud sessions in the CLI.",
      label: "Update device access to search and read Cloud sessions",
    },
    {
      action: "capture",
      show: value.captureUpdateAvailable,
      description: "Approve this device to upload sessions.",
      label: "Update device access to upload sessions",
    },
  ] as const
  return (
    <>
      {value.accessUpdate && (
        <div className={row}>
          <p className={text} role="status">
            {value.accessUpdate === "denied"
              ? "Device access did not change."
              : "Could not update device access. Your current connection is still active. Try again."}
          </p>
        </div>
      )}
      {options
        .filter((option) => option.show)
        .map(({ action, description, label }) => (
          <div key={action} className={row}>
            <p className={text}>{description}</p>
            <button
              type="button"
              className={button}
              aria-label={label}
              disabled={busy}
              onClick={() => void update(action)}
            >
              Update device access
            </button>
          </div>
        ))}
    </>
  )
}
