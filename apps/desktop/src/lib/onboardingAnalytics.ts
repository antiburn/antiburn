import { noteInteraction } from "./ipc"

/** Closed screen labels shared with native analytics; never use account/source text. */
export type OnboardingAnalyticsScreen =
  | "welcome"
  | "sessions"
  | "limits"
  | "hud"
  | "checks"
  | "cloud"
  | "cloudUnavailable"
  | "cloudHandoff"
  | "cloudIdentity"
  | "cloudFolders"
  | "cloudSync"
  | "cloudComplete"

/** Explicit navigation and setup intent, not proof of upload or content acceptance. */
export type OnboardingAnalyticsAction =
  "get_started" | "continue" | "back" | "use_locally" | "set_up_cloud" | CloudAnalyticsAction

/** Safe renderer interactions; native code owns verified outcomes and durable completion. */
export type OnboardingInteraction =
  | { kind: "onboardingScreenViewed"; screen: OnboardingAnalyticsScreen }
  | {
      kind: "onboardingActionTaken"
      screen: OnboardingAnalyticsScreen
      action: OnboardingAnalyticsAction
    }
  | { kind: "onboardingCloudUnavailable" }

/** Record visible entries only; callers deduplicate renders and hidden surfaces. */
export function noteOnboardingScreenViewed(screen: OnboardingAnalyticsScreen): void {
  noteInteraction({ kind: "onboardingScreenViewed", screen })
}

/** Record accepted user intent without adding source names, counts, or credentials. */
export function noteOnboardingAction(
  screen: OnboardingAnalyticsScreen,
  action: OnboardingAnalyticsAction,
): void {
  noteInteraction({ kind: "onboardingActionTaken", screen, action })
}

/** Preserve the legacy unavailable-screen event, not a current build-feature probe. */
export function noteOnboardingCloudUnavailable(): void {
  noteInteraction({ kind: "onboardingCloudUnavailable" })
}

/** Nested Cloud labels. The old sync label remains for isolated/legacy presentation. */
export type CloudAnalyticsScreen = "handoff" | "identity" | "folders" | "sync" | "complete"
/** Setup choices only; saving intent never reports capture or upload success. */
export type CloudAnalyticsAction =
  | "use_locally"
  | "learn_more"
  | "sign_in"
  | "cancel"
  | "retry"
  | "back"
  | "continue"
  | "add_folder"
  | "select_folder"
  | "start_sync"
  | "check_status"
  | "cancel_sync"
  | "done"
  | "sign_out"

const cloudScreens = {
  handoff: "cloudHandoff",
  identity: "cloudIdentity",
  folders: "cloudFolders",
  sync: "cloudSync",
  complete: "cloudComplete",
} as const satisfies Record<CloudAnalyticsScreen, OnboardingAnalyticsScreen>

/** Translate a deduplicated visible Cloud entry to the fixed public screen label. */
export function noteCloudScreenViewed(screen: CloudAnalyticsScreen): void {
  noteOnboardingScreenViewed(cloudScreens[screen])
}

/** Translate explicit Cloud/local choice without emitting native operation outcomes. */
export function noteCloudAction(
  screen: CloudAnalyticsScreen,
  action: CloudAnalyticsAction,
): void {
  noteOnboardingAction(cloudScreens[screen], action)
}
