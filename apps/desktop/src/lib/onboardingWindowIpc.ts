import { invoke } from "@tauri-apps/api/core"
import { isMacOS } from "./platform"
import { hasShell } from "./ipc"
import { listen } from "./tauriEvents"

export async function getOnboardingWindowVisible(): Promise<boolean> {
  if (!hasShell()) return !document.hidden
  return invoke<boolean>("get_onboarding_window_visible")
}

export async function onOnboardingWindowVisibilityChanged(
  handler: (visible: boolean) => void,
): Promise<() => void> {
  if (!hasShell()) {
    const change = () => handler(!document.hidden)
    document.addEventListener("visibilitychange", change)
    return () => document.removeEventListener("visibilitychange", change)
  }
  return listen<boolean>("onboarding:visibility", (event) => handler(event.payload))
}

export type OnboardingHudVariant = "notch" | "floating" | "unsupported"

export async function getOnboardingHudVariant(): Promise<OnboardingHudVariant> {
  if (!isMacOS()) return "unsupported"
  if (!hasShell()) return "floating"
  const available = await invoke<boolean>("hud_island_available").catch(() => false)
  return available ? "notch" : "floating"
}
