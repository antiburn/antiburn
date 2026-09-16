import type { LiveUsageProvider } from "./ipc"
import { hasWorkingActivity, type LiveSessionsSnapshot } from "./sessionLifecycle"

const AGENT_PROVIDERS: Readonly<Record<string, LiveUsageProvider>> = {
  "claude-code": "anthropic",
  codex: "openai",
  antigravity: "google",
}

export type ProviderModels = Readonly<Partial<Record<string, readonly string[]>>>

export const isLive = hasWorkingActivity

/** Provider counts include anonymous activity but never infer models. */
export function liveProviders(state: LiveSessionsSnapshot): LiveUsageProvider[] {
  return [
    ...new Set(
      state.sweep.flatMap((count) => {
        const provider = AGENT_PROVIDERS[count.agent]
        return provider && count.working + count.anonymous > 0 ? [provider] : []
      }),
    ),
  ].sort()
}

/** Positive model evidence remains inside its agent's provider. */
export function liveModels(state: LiveSessionsSnapshot): ProviderModels {
  const models = new Map<LiveUsageProvider, Set<string>>()
  for (const count of state.sweep) {
    const provider = AGENT_PROVIDERS[count.agent]
    if (!provider) continue
    for (const model of count.models) {
      if (model.working <= 0) continue
      let set = models.get(provider)
      if (!set) models.set(provider, (set = new Set()))
      set.add(model.model)
    }
  }
  return Object.fromEntries(
    [...models]
      .sort(([a], [b]) => a.localeCompare(b))
      .map(([provider, values]) => [provider, [...values].sort()]),
  )
}

export function sameProviderModels(left: ProviderModels, right: ProviderModels): boolean {
  return JSON.stringify(left) === JSON.stringify(right)
}
