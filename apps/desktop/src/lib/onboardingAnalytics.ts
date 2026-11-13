import { noteInteraction } from "./ipc"

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

export type OnboardingAnalyticsAction =
  "get_started" | "continue" | "back" | "use_locally" | "set_up_cloud" | CloudAnalyticsAction

export type OnboardingInteraction =
  | { kind: "onboardingScreenViewed"; screen: OnboardingAnalyticsScreen }
  | {
      kind: "onboardingActionTaken"
      screen: OnboardingAnalyticsScreen
      action: OnboardingAnalyticsAction
    }
  | { kind: "onboardingCloudUnavailable" }

export function noteOnboardingScreenViewed(screen: OnboardingAnalyticsScreen): void {
  noteInteraction({ kind: "onboardingScreenViewed", screen })
}

export function noteOnboardingAction(
  screen: OnboardingAnalyticsScreen,
  action: OnboardingAnalyticsAction,
): void {
  noteInteraction({ kind: "onboardingActionTaken", screen, action })
}

export function noteOnboardingCloudUnavailable(): void {
  noteInteraction({ kind: "onboardingCloudUnavailable" })
}

export type CloudAnalyticsScreen = "handoff" | "identity" | "folders" | "sync" | "complete"
export type CloudAnalyticsAction =
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

export function noteCloudScreenViewed(screen: CloudAnalyticsScreen): void {
  noteOnboardingScreenViewed(cloudScreens[screen])
}

export function noteCloudAction(
  screen: CloudAnalyticsScreen,
  action: CloudAnalyticsAction,
): void {
  noteOnboardingAction(cloudScreens[screen], action)
}
