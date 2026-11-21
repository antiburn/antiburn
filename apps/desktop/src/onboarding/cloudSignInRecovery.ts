import type { CloudSnapshot } from "../lib/cloud"
import { cloudStatusMessage } from "./CloudSession"

/** Describe the saved connection that must be handled before another sign-in. */
export function cloudSignInRecovery(cloud: CloudSnapshot) {
  if (cloud.localCleanupPending) {
    return {
      title: "Sign-out incomplete",
      message: "Desktop still needs to clear your saved sign-in.",
      action: "Finish signing out",
    }
  }
  if (cloud.revocationPending) {
    return {
      title: "Sign-out incomplete",
      message: "Cloud hasn’t confirmed that your previous connection is closed.",
      action: "Retry sign-out",
    }
  }
  if (cloud.status === "reauth_required" || cloud.status === "credential_failed") {
    return {
      title: "Sign-in needs attention",
      message: cloudStatusMessage(cloud.status),
      action: "Sign out of previous connection",
    }
  }
  return null
}
