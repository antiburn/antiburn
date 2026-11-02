import { ChevronRight, LoaderCircle, RefreshCw, Wrench } from "lucide-react"
import { useRef, useState } from "react"

import { PushButton } from "../../components/ui/PushButton"
import type { IntegrationStatus } from "../../lib/agentIntegrationsIpc"
import { agentIntegrationsSession } from "../../lib/agentIntegrationsSession"

const sharedReasons = new Set([
  "development_build",
  "startup_pending",
  "compatible_node_22_required",
  "managed_cli_unavailable",
])

export function integrationBadge(
  status: IntegrationStatus,
  name: string,
): { tone: "success" | "attention" | "neutral"; description: string } | null {
  const host = status.vendor === "claude" ? "Claude Code" : name
  if (sharedReasons.has(status.reason)) return null
  if (status.owned && ["registered", "active"].includes(status.phase)) {
    return {
      tone: "success",
      description: `Antiburn is integrated with ${host}. Open Integration for details.`,
    }
  }
  if (["awaiting_trust", "reload_required", "user_changed"].includes(status.phase)) {
    const action =
      status.phase === "awaiting_trust"
        ? "needs approval"
        : status.phase === "reload_required"
          ? "needs a restart or reload"
          : "needs attention"
    return {
      tone: "attention",
      description: `Antiburn integration in ${host} ${action}. Open Integration for the next step.`,
    }
  }
  if (
    status.phase === "not_detected" ||
    status.phase === "disabled" ||
    (status.phase === "unavailable" && status.reason === "setup_not_registered")
  ) {
    return {
      tone: "neutral",
      description:
        status.phase === "disabled"
          ? `Integration available — turned off. Open Integration for ${host} details.`
          : `Integration available — not set up. Open Integration for ${host} details.`,
    }
  }
  return null
}

function integrationDescription(status: IntegrationStatus, name = "your agent"): string {
  if (status.reason === "development_build")
    return "Integration setup is available in packaged builds."
  if (status.reason === "startup_pending") return "Antiburn is preparing integrations…"
  if (status.reason === "compatible_node_22_required")
    return "Antiburn needs Node.js 22 or later. Install it, then reopen Antiburn."
  if (status.reason === "managed_cli_unavailable")
    return "Antiburn’s internal runtime is unavailable. Repair it in CLI settings. This status updates automatically."
  if (status.phase === "unavailable" && status.reason === "setup_not_registered")
    return `Antiburn integration hasn’t been set up for ${name}. Reopen Antiburn to retry automatic setup.`
  switch (status.phase) {
    case "not_detected":
      return `Setup files for ${name} weren’t found. They must be available before integration can be set up.`
    case "pending_native_gate":
      return `Antiburn integration is not available for this version of ${name} yet.`
    case "awaiting_trust":
      return `Approve Antiburn in ${name}, then restart or reload ${name} to finish setup.`
    case "reload_required":
      return `Restart or reload ${name} to finish setting up the integration.`
    case "registered":
    case "active":
      return status.owned
        ? `Antiburn is integrated with ${name}.`
        : `Antiburn integration in ${name} could not be confirmed. Refresh setup to check again.`
    case "disabled":
      return `Antiburn integration is off for ${name}.`
    case "user_changed":
      return `Antiburn setup in ${name} has changed. Your edits have been kept.`
    default:
      return `Antiburn could not set up integration with ${name}. Check its installation and access to its settings.`
  }
}

export function integrationTitle(status: IntegrationStatus): string {
  if (status.reason === "startup_pending") return "Preparing integration"
  if (sharedReasons.has(status.reason)) return "Setup unavailable"
  if (status.phase === "unavailable" && status.reason === "setup_not_registered")
    return "Integration isn’t set up."
  switch (status.phase) {
    case "not_detected":
      return "Integration isn’t set up."
    case "awaiting_trust":
      return "Approval required"
    case "reload_required":
      return "Restart required"
    case "registered":
    case "active":
      return status.owned ? "Integration connected." : "Setup needs attention"
    case "disabled":
      return "Integration off"
    case "user_changed":
      return "Setup changed"
    default:
      return "Setup unavailable"
  }
}

export function AgentConnectionDetails({
  status,
  name,
  busy,
  onOpenCli,
  onFocusIntegration,
}: {
  status: IntegrationStatus
  name: string
  busy: boolean
  onOpenCli: () => void
  onFocusIntegration: () => void
}) {
  const host = status.vendor === "claude" ? "Claude Code" : name
  const tone = integrationBadge(status, name)?.tone
  const [pendingAction, setPendingAction] = useState<"refresh" | "repair" | null>(null)
  const [feedback, setFeedback] = useState<{
    status: IntegrationStatus
    message: string
  } | null>(null)
  const feedbackMessage =
    feedback &&
    (Object.keys(status) as (keyof IntegrationStatus)[]).every(
      (key) => status[key] === feedback.status[key],
    )
      ? feedback.message
      : ""
  const refreshing = pendingAction === "refresh"
  const repairing = pendingAction === "repair"
  const RepairIcon = repairing ? LoaderCircle : Wrench
  const actionPending = useRef(false)
  const canRepair =
    (status.owned || status.phase === "user_changed") &&
    !["not_detected", "pending_native_gate"].includes(status.phase) &&
    !sharedReasons.has(status.reason)
  const canRefresh =
    !sharedReasons.has(status.reason) &&
    !["unavailable", "pending_native_gate"].includes(status.phase)
  const connected =
    status.owned &&
    ["registered", "active"].includes(status.phase) &&
    !sharedReasons.has(status.reason)
  const needsApproval = status.phase === "awaiting_trust" && !sharedReasons.has(status.reason)

  async function runAction(
    action: () => Promise<void>,
    trigger: HTMLButtonElement | null,
    operation: "refresh" | "repair",
  ) {
    if (actionPending.current || busy || agentIntegrationsSession.getSnapshot().busy) return
    actionPending.current = true
    setPendingAction(operation)
    setFeedback(null)
    try {
      await action()
      const snapshot = agentIntegrationsSession.getSnapshot()
      const next = snapshot.statuses.find((entry) => entry.vendor === status.vendor)
      if (!snapshot.error && next) {
        const unchanged = (Object.keys(status) as (keyof IntegrationStatus)[]).every(
          (key) => status[key] === next[key],
        )
        setFeedback({
          status: next,
          message:
            operation === "refresh"
              ? unchanged
                ? "Status checked — no changes."
                : "Status updated."
              : next.phase === "reload_required"
                ? "Repair finished. Restart or reload is still required."
                : next.phase === "awaiting_trust"
                  ? "Repair finished. Approval is still required."
                  : next.owned && (next.phase === "registered" || next.phase === "active")
                    ? "Repair finished. Integration is connected."
                    : "Repair finished. Review the integration status above.",
        })
        if (
          !unchanged &&
          trigger &&
          (document.activeElement === trigger || document.activeElement === document.body)
        )
          onFocusIntegration()
      }
    } finally {
      actionPending.current = false
      setPendingAction(null)
    }
  }

  return (
    <section
      className="space-y-3 type-footnote"
      aria-label={`${name} Antiburn integration`}
      aria-busy={busy}
    >
      <div className="space-y-2">
        <p
          className={`font-semibold! ${tone === "attention" ? "agent-settings-badge-attention" : tone === "success" ? "agent-settings-badge-success" : "text-label"}`}
        >
          {integrationTitle(status)}
        </p>
        {needsApproval ? (
          <ol className="list-decimal space-y-1 pl-5 text-label-secondary">
            <li>Open {host}.</li>
            <li>Approve Antiburn when prompted.</li>
            <li>Restart or reload {host}.</li>
          </ol>
        ) : !connected ? (
          <p className="text-pretty text-label-secondary">
            {integrationDescription(status, host)}
          </p>
        ) : null}
        {status.reason === "managed_cli_unavailable" && (
          <PushButton onClick={onOpenCli}>Open CLI settings</PushButton>
        )}
        {(canRefresh || canRepair) && (
          <div
            role="group"
            aria-label={`${name} integration maintenance`}
            className="flex flex-wrap items-center gap-x-2 gap-y-6 pt-1"
          >
            {canRefresh && (
              <button
                type="button"
                aria-label={`Refresh ${name} integration status`}
                aria-busy={refreshing}
                className="agent-integration-maintenance ui-push-button gap-1.5 whitespace-nowrap aria-disabled:opacity-50"
                aria-disabled={busy}
                onClick={(event) => {
                  if (!busy)
                    void runAction(
                      () => agentIntegrationsSession.refresh(),
                      event.currentTarget,
                      "refresh",
                    )
                }}
              >
                <RefreshCw
                  size={12}
                  aria-hidden="true"
                  className={refreshing ? "motion-safe:animate-spin" : undefined}
                />
                Refresh status
              </button>
            )}
            {canRepair && (
              <button
                type="button"
                aria-label={`Repair ${name} integration`}
                aria-busy={repairing}
                className="agent-integration-repair agent-integration-maintenance ui-push-button gap-1.5 whitespace-nowrap aria-disabled:opacity-50"
                aria-disabled={busy}
                onClick={(event) => {
                  if (!busy)
                    void runAction(
                      () => agentIntegrationsSession.repair(status.vendor),
                      event.currentTarget,
                      "repair",
                    )
                }}
              >
                <RepairIcon
                  size={12}
                  aria-hidden="true"
                  className={repairing ? "shrink-0 motion-safe:animate-spin" : "shrink-0"}
                />
                <span>{repairing ? "Repairing…" : "Repair setup"}</span>
              </button>
            )}
          </div>
        )}
        <p
          role="status"
          aria-atomic="true"
          className="min-h-lh type-caption text-label-secondary"
        >
          {refreshing
            ? "Checking integration status…"
            : repairing
              ? "Repairing integration setup…"
              : feedbackMessage}
        </p>
      </div>
      <details className="group border-t border-separator">
        <summary className="agent-integration-maintenance flex cursor-default list-none items-center gap-1.5 rounded-control text-label-secondary hover:text-label [&::-webkit-details-marker]:hidden">
          <ChevronRight size={12} className="group-open:rotate-90" aria-hidden="true" />
          Technical details
        </summary>
        <div className="space-y-3 pb-1">
          <dl className="grid grid-cols-[auto_minmax(0,1fr)] gap-x-3 gap-y-1 type-caption">
            <dt className="text-label-secondary">Method</dt>
            <dd className="font-medium! text-label">
              {status.vendor === "open_code"
                ? "Plugin"
                : status.vendor === "pi"
                  ? "Extension"
                  : "Hooks"}
            </dd>
            {status.hostVersion && (
              <>
                <dt className="text-label-secondary">Agent version</dt>
                <dd className="break-words font-medium! text-label">{status.hostVersion}</dd>
              </>
            )}
            {status.nodeVersion && (
              <>
                <dt className="text-label-secondary">Node.js</dt>
                <dd className="break-words font-medium! text-label">{status.nodeVersion}</dd>
              </>
            )}
            {status.profile && (
              <>
                <dt className="text-label-secondary">Profile</dt>
                <dd className="select-text break-all font-mono font-medium! text-label">
                  {status.profile}
                </dd>
              </>
            )}
          </dl>
          {status.vendor === "claude" && (
            <p className="type-caption text-label-tertiary">
              The public Claude usage plugin is managed separately.
            </p>
          )}
        </div>
      </details>
    </section>
  )
}
