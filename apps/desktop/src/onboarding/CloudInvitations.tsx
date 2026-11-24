import { useState, useSyncExternalStore } from "react"
import { PushButton } from "../components/ui/PushButton"
import {
  desktopInvitationPort,
  type CloudInvitationPort,
  type InvitationReply,
  type InvitationState,
} from "./cloudInvitationPort"

const POLL_INTERVAL_MS = 2_000
const POLL_WINDOW_MS = 5 * 60_000

type View = {
  reply: InvitationReply | null
  busy: boolean
  failed: boolean
  hasJoinAttempt: boolean
}

const initialView: View = {
  reply: null,
  busy: false,
  failed: false,
  hasJoinAttempt: false,
}

/** Observe only while mounted. Cancel and finish are explicit actions, not poll side effects. */
class InvitationView {
  private view = initialView
  private listeners = new Set<() => void>()
  private timer: ReturnType<typeof setTimeout> | undefined
  private deadline = 0
  private revision = 0
  private readonly accountId: string
  private readonly port: CloudInvitationPort
  private readonly onJoined: () => void
  private readonly showConfirmedOnly: boolean

  constructor(
    accountId: string,
    port: CloudInvitationPort,
    onJoined: () => void,
    showConfirmedOnly: boolean,
  ) {
    this.accountId = accountId
    this.port = port
    this.onJoined = onJoined
    this.showConfirmedOnly = showConfirmedOnly
  }

  getSnapshot = () => this.view

  subscribe = (listener: () => void) => {
    this.listeners.add(listener)
    if (this.listeners.size === 1) void this.send({ action: "list" }, this.showConfirmedOnly)
    return () => {
      this.listeners.delete(listener)
      if (this.listeners.size === 0) {
        clearTimeout(this.timer)
        this.revision++
        this.view = initialView
      }
    }
  }

  private publish(patch: Partial<View>) {
    this.view = { ...this.view, ...patch }
    for (const listener of this.listeners) listener()
  }

  send = async (request: Parameters<CloudInvitationPort>[1], retryOnce = false) => {
    if (this.view.busy) return
    clearTimeout(this.timer)
    const revision = ++this.revision
    if (request.action === "start" || request.action === "list") {
      this.deadline = performance.now() + POLL_WINDOW_MS
    }
    const polling = request.action === "status" && isWaiting(this.view.reply)
    if (polling && performance.now() >= this.deadline) {
      this.publish({ reply: { kind: "expired" }, failed: false })
      return
    }
    if (!polling)
      this.publish({ busy: true, failed: this.showConfirmedOnly && this.view.failed })
    let reply: InvitationReply
    try {
      reply = await this.port(this.accountId, request)
    } catch {
      if (revision !== this.revision) return
      const attempt = this.view.reply
      if (
        (request.action === "finish" || request.action === "cancel") &&
        attempt?.kind === "attempt"
      ) {
        try {
          reply = await this.port(this.accountId, { action: "status", id: request.id })
        } catch {
          reply = { kind: "uncertain", id: request.id, join: attempt.join }
        }
      } else {
        const hiddenDismiss = this.showConfirmedOnly && request.action === "dismiss" && !attempt
        this.publish({ busy: false, failed: !hiddenDismiss })
        if (isWaiting(attempt)) this.schedule(attempt.id)
        if (
          revision === this.revision &&
          retryOnce &&
          (request.action === "list" || hiddenDismiss)
        ) {
          this.timer = setTimeout(() => void this.send(request), POLL_INTERVAL_MS)
        }
        return
      }
    }
    if (revision !== this.revision) return
    const previous = this.view.reply
    if (
      this.showConfirmedOnly &&
      !previous &&
      request.action === "list" &&
      reply.kind === "attempt" &&
      ["canceled", "no_seat", "closed", "wrong_account", "unavailable"].includes(reply.state)
    ) {
      // Retire an old failure without showing it as a pending invitation.
      this.publish({ busy: false, failed: false })
      if (revision === this.revision) void this.send({ action: "dismiss", id: reply.id }, true)
      return
    }
    // Expiry and sign-in replies do not say whether the current attempt is a join.
    const hasJoinAttempt =
      reply.kind === "attempt" || reply.kind === "uncertain"
        ? reply.join
        : reply.kind !== "list" && this.view.hasJoinAttempt
    this.publish({ reply, busy: false, failed: false, hasJoinAttempt })
    if (isWaiting(reply)) this.schedule(reply.id)
    if (reply.kind !== "attempt" || request.action === "dismiss") return
    const joined = reply.state === "joined"
    // Dismiss retires a completed Desktop attempt and reads the list. It cannot join a workspace.
    if (
      this.showConfirmedOnly &&
      (reply.join ? joined : reply.state === "checked") &&
      revision === this.revision
    )
      void this.send({ action: "dismiss", id: reply.id })
    if (
      joined &&
      (previous?.kind !== "attempt" || previous.state !== "joined" || previous.id !== reply.id)
    )
      this.onJoined()
  }

  private schedule(id: string) {
    this.timer = setTimeout(() => void this.send({ action: "status", id }), POLL_INTERVAL_MS)
  }
}

function isWaiting(
  reply: InvitationReply | null,
): reply is Extract<InvitationReply, { kind: "attempt" }> & { state: "waiting" } {
  return reply?.kind === "attempt" && reply.state === "waiting"
}

const attemptMessage: Record<InvitationState, string> = {
  waiting: "Verify your account in your browser, then return to Desktop.",
  ready: "Account verified. Join the workspace to accept the invitation.",
  checked: "Account verified. Review your invitations.",
  unavailable: "Could not check invitations for this account.",
  wrong_account: "Your browser used a different account. Try again with your Desktop account.",
  canceled: "Invitation verification canceled.",
  joined: "You joined the workspace.",
  no_seat: "This workspace has no open seats. Ask a workspace admin for help.",
  closed: "This invitation is no longer open.",
}

function workspaceLabel(name: string | null | undefined, slug: string | null | undefined) {
  return name && slug ? `${name} (${slug})` : name || slug
}

/** Render inside a signed-in Cloud surface. Key the view by account and reload
 * destinations on join. This view does not start sync or block local use.
 */
export function CloudInvitations({
  accountId,
  onJoined,
  port = desktopInvitationPort,
  hideEmpty = false,
  showConfirmedOnly = false,
}: {
  accountId: string
  onJoined: () => void
  port?: CloudInvitationPort
  /** Hide a confirmed empty list in onboarding, not verification or retry. */
  hideEmpty?: boolean
  /** Show only confirmed invitations and recovery for a join attempt in Settings. */
  showConfirmedOnly?: boolean
}) {
  return (
    <AccountInvitations
      key={`${accountId}:${showConfirmedOnly}`}
      accountId={accountId}
      port={port}
      onJoined={onJoined}
      hideEmpty={hideEmpty}
      showConfirmedOnly={showConfirmedOnly}
    />
  )
}

function AccountInvitations({
  accountId,
  port,
  onJoined,
  hideEmpty,
  showConfirmedOnly,
}: {
  accountId: string
  port: CloudInvitationPort
  onJoined: () => void
  hideEmpty: boolean
  showConfirmedOnly: boolean
}) {
  const [controller] = useState(
    () => new InvitationView(accountId, port, onJoined, showConfirmedOnly),
  )
  const { reply, busy, failed, hasJoinAttempt } = useSyncExternalStore(
    controller.subscribe,
    controller.getSnapshot,
    controller.getSnapshot,
  )
  const send = (request: Parameters<CloudInvitationPort>[1]) => void controller.send(request)
  const ready = reply?.kind === "attempt" && reply.state === "ready"
  const waiting = isWaiting(reply)
  const joined = reply?.kind === "attempt" && reply.state === "joined"
  const refresh = () =>
    send(
      reply?.kind === "attempt" && !ready && !waiting && !(showConfirmedOnly && joined)
        ? { action: "dismiss", id: reply.id }
        : { action: "list" },
    )
  const workspace =
    reply?.kind === "attempt"
      ? workspaceLabel(reply.workspace_name, reply.workspace_slug)
      : null
  const finishLabel = ready && reply.join ? "Join workspace" : "Finish check"
  const listing = reply?.kind === "list" ? reply.listing : null
  const hasInvitations = listing?.state === "checked" && listing.invitations.length > 0
  if (showConfirmedOnly && !hasJoinAttempt && !hasInvitations) return null
  if (hideEmpty && !failed && listing?.state === "checked" && !listing.invitations.length)
    return null
  const showBrowserCheck = listing?.state === "check_required"
  const showRefresh =
    !ready &&
    !waiting &&
    reply?.kind !== "reauth_required" &&
    reply?.kind !== "uncertain" &&
    !!(reply || failed) &&
    (!showConfirmedOnly || failed || (!hasInvitations && !joined))
  const message = failed
    ? showConfirmedOnly && joined
      ? "You joined the workspace. Could not refresh invitations. Try again."
      : "Could not check Cloud invitations. Try again or continue without joining."
    : reply?.kind === "attempt"
      ? reply.state === "ready"
        ? reply.join
          ? workspace
            ? `Account verified. Join ${workspace} to accept the invitation.`
            : attemptMessage.ready
          : "Account verified. Finish checking for invitations."
        : attemptMessage[reply.state]
      : reply?.kind === "uncertain"
        ? "Cloud has not confirmed the result. Check the same attempt before trying again."
        : reply?.kind === "reauth_required"
          ? reply.uncertain
            ? "Cloud has not confirmed the result. Check your Cloud workspaces before trying again."
            : "Reconnect to Cloud to check workspace invitations."
          : reply?.kind === "expired"
            ? reply.uncertain
              ? "Verification timed out before Cloud confirmed the result. Check your workspaces and invitations before trying again."
              : "Verification timed out. You can try again."
            : listing?.state === "check_required"
              ? "Verify your account in your browser to check for invitations."
              : listing?.state === "unavailable"
                ? "Workspace invitations are unavailable."
                : listing?.state === "checked" && listing.invitations.length === 0
                  ? "No pending workspace invitations."
                  : !reply && busy
                    ? "Checking for workspace invitations…"
                    : ""
  return (
    <section aria-label="Workspace invitations" className="flex min-w-0 flex-col gap-2">
      <h2 className="type-callout font-medium">Workspace invitations</h2>
      {(!showConfirmedOnly || message) && (
        <p className="min-w-0 break-words type-footnote text-label-secondary" role="status">
          {message}
        </p>
      )}
      {hasInvitations && (
        <ul className="flex max-h-40 min-w-0 flex-col gap-2 overflow-y-auto">
          {listing.invitations.map((invitation) => {
            const label = workspaceLabel(invitation.workspace_name, invitation.workspace_slug)
            return (
              <li
                className="flex min-w-0 flex-wrap items-center justify-between gap-2"
                key={invitation.id}
              >
                <span className="min-w-0 break-all type-callout">{label}</span>
                <PushButton
                  ariaLabel={
                    showConfirmedOnly ? `Join ${label}` : `Verify invitation to ${label}`
                  }
                  disabled={busy}
                  onClick={() =>
                    send({
                      action: "start",
                      invitation_id: invitation.id,
                      workspace_name: invitation.workspace_name,
                      workspace_slug: invitation.workspace_slug,
                    })
                  }
                >
                  {showConfirmedOnly ? "Join" : "Verify invitation"}
                </PushButton>
              </li>
            )
          })}
        </ul>
      )}
      {(!showConfirmedOnly ||
        showBrowserCheck ||
        ready ||
        waiting ||
        reply?.kind === "uncertain" ||
        showRefresh) && (
        <div className="flex flex-wrap gap-2">
          {showBrowserCheck && (
            <PushButton
              disabled={busy}
              onClick={() => send({ action: "start", invitation_id: null })}
            >
              Check in browser
            </PushButton>
          )}
          {ready && (
            <PushButton
              variant="primary"
              ariaLabel={reply.join && workspace ? `${finishLabel} ${workspace}` : finishLabel}
              disabled={busy}
              onClick={() => send({ action: "finish", id: reply.id })}
            >
              {finishLabel}
            </PushButton>
          )}
          {(ready || waiting) && (
            <PushButton
              disabled={busy}
              onClick={() => send({ action: "cancel", id: reply.id })}
            >
              Cancel verification
            </PushButton>
          )}
          {reply?.kind === "uncertain" && (
            <PushButton
              disabled={busy}
              onClick={() => send({ action: "status", id: reply.id })}
            >
              Check result
            </PushButton>
          )}
          {showRefresh && (
            <PushButton disabled={busy} onClick={refresh}>
              {reply?.kind === "attempt" && reply.state === "checked"
                ? "Review invitations"
                : "Refresh invitations"}
            </PushButton>
          )}
        </div>
      )}
    </section>
  )
}
