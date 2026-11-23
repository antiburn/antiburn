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
  error: string | null
}

/** Observe only while mounted. Cancel and finish are explicit actions, not poll side effects. */
class InvitationView {
  private view: View = { reply: null, busy: false, error: null }
  private listeners = new Set<() => void>()
  private timer: ReturnType<typeof setTimeout> | undefined
  private deadline = 0
  private revision = 0
  private readonly accountId: string
  private readonly port: CloudInvitationPort
  private readonly onJoined: () => void

  constructor(accountId: string, port: CloudInvitationPort, onJoined: () => void) {
    this.accountId = accountId
    this.port = port
    this.onJoined = onJoined
  }

  getSnapshot = () => this.view

  subscribe = (listener: () => void) => {
    this.listeners.add(listener)
    if (this.listeners.size === 1) void this.send({ action: "list" })
    return () => {
      this.listeners.delete(listener)
      if (this.listeners.size === 0) {
        clearTimeout(this.timer)
        this.revision++
        this.view = { reply: null, busy: false, error: null }
      }
    }
  }

  private publish(patch: Partial<View>) {
    this.view = { ...this.view, ...patch }
    for (const listener of this.listeners) listener()
  }

  send = async (request: Parameters<CloudInvitationPort>[1]) => {
    if (this.view.busy) return
    clearTimeout(this.timer)
    const revision = ++this.revision
    if (request.action === "start" || request.action === "list") {
      this.deadline = performance.now() + POLL_WINDOW_MS
    }
    const polling = request.action === "status" && isWaiting(this.view.reply)
    if (polling && performance.now() >= this.deadline) {
      this.publish({ reply: { kind: "expired" }, error: null })
      return
    }
    if (!polling) this.publish({ busy: true, error: null })
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
        this.publish({
          busy: false,
          error: "Could not check Cloud invitations. Try again or continue without joining.",
        })
        if (isWaiting(attempt)) this.schedule(attempt.id)
        return
      }
    }
    if (revision !== this.revision) return
    this.publish({ reply, busy: false, error: null })
    if (isWaiting(reply)) this.schedule(reply.id)
    if (reply.kind === "attempt" && reply.state === "joined") this.onJoined()
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
}: {
  accountId: string
  onJoined: () => void
  port?: CloudInvitationPort
}) {
  return (
    <AccountInvitations key={accountId} accountId={accountId} port={port} onJoined={onJoined} />
  )
}

function AccountInvitations({
  accountId,
  port,
  onJoined,
}: {
  accountId: string
  port: CloudInvitationPort
  onJoined: () => void
}) {
  const [controller] = useState(() => new InvitationView(accountId, port, onJoined))
  const { reply, busy, error } = useSyncExternalStore(
    controller.subscribe,
    controller.getSnapshot,
    controller.getSnapshot,
  )
  const send = (request: Parameters<CloudInvitationPort>[1]) => void controller.send(request)
  const ready = reply?.kind === "attempt" && reply.state === "ready"
  const waiting = isWaiting(reply)
  const refresh = () =>
    send(
      reply?.kind === "attempt" && !ready && !waiting
        ? { action: "dismiss", id: reply.id }
        : { action: "list" },
    )
  const workspace =
    reply?.kind === "attempt"
      ? workspaceLabel(reply.workspace_name, reply.workspace_slug)
      : null
  const finishLabel = ready && reply.join ? "Join workspace" : "Finish check"
  const listing = reply?.kind === "list" ? reply.listing : null
  const message = error
    ? error
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
      <p className="min-w-0 break-words type-footnote text-label-secondary" role="status">
        {message}
      </p>
      {listing?.state === "checked" && listing.invitations.length > 0 && (
        <ul className="flex max-h-40 min-w-0 flex-col gap-2 overflow-y-auto">
          {listing.invitations.map((invitation) => (
            <li
              className="flex min-w-0 flex-wrap items-center justify-between gap-2"
              key={invitation.id}
            >
              <span className="min-w-0 break-all type-callout">
                {workspaceLabel(invitation.workspace_name, invitation.workspace_slug)}
              </span>
              <PushButton
                ariaLabel={`Verify invitation to ${workspaceLabel(invitation.workspace_name, invitation.workspace_slug)}`}
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
                Verify invitation
              </PushButton>
            </li>
          ))}
        </ul>
      )}
      <div className="flex flex-wrap gap-2">
        {listing?.state === "check_required" && (
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
          <PushButton disabled={busy} onClick={() => send({ action: "cancel", id: reply.id })}>
            Cancel verification
          </PushButton>
        )}
        {reply?.kind === "uncertain" ? (
          <PushButton disabled={busy} onClick={() => send({ action: "status", id: reply.id })}>
            Check result
          </PushButton>
        ) : (
          !waiting &&
          !ready &&
          reply?.kind !== "reauth_required" &&
          (reply || error) && (
            <PushButton disabled={busy} onClick={refresh}>
              {reply?.kind === "attempt" && reply.state === "checked"
                ? "Review invitations"
                : "Refresh invitations"}
            </PushButton>
          )
        )}
      </div>
    </section>
  )
}
