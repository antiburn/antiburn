import { invoke } from "@tauri-apps/api/core"

type Invitation = {
  id: string
  workspace_name: string
  workspace_slug: string
}

type InvitationListing =
  { state: "checked"; invitations: Invitation[] } | { state: "check_required" | "unavailable" }

export type InvitationState =
  | "waiting"
  | "ready"
  | "checked"
  | "unavailable"
  | "wrong_account"
  | "canceled"
  | "joined"
  | "no_seat"
  | "closed"

type InvitationAction =
  | { action: "list" }
  | {
      action: "start"
      invitation_id: string | null
      workspace_name?: string
      workspace_slug?: string
    }
  | { action: "status" | "finish" | "cancel" | "dismiss"; id: string }

export type InvitationReply =
  | { kind: "list"; listing: InvitationListing }
  | {
      kind: "attempt"
      id: string
      join: boolean
      state: InvitationState
      workspace_id: string | null
      workspace_name?: string | null
      workspace_slug?: string | null
    }
  | { kind: "uncertain"; id: string; join: boolean }
  | { kind: "expired"; uncertain?: boolean }
  | { kind: "reauth_required"; uncertain: boolean }

/** The renderer sends intent only. Desktop owns the account check, ticket, and browser. */
export type CloudInvitationPort = (
  accountId: string,
  request: InvitationAction,
) => Promise<InvitationReply>

export const desktopInvitationPort: CloudInvitationPort = (accountId, request) =>
  invoke("cloud_invitations", { accountId, request })
