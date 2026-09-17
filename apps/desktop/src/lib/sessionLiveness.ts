import type { LiveUsageProvider } from "./ipc"
import { hasWorkingActivity, type LiveSessionsSnapshot } from "./sessionLifecycle"

function displayedProvider(route: string | null): LiveUsageProvider | null {
  return route === "anthropic" || route === "openai" || route === "google" ? route : null
}

export type ProviderModels = Readonly<Partial<Record<string, readonly string[]>>>

export const isLive = hasWorkingActivity

/** Only recorded canonical routes activate provider meters. */
export function liveProviders(state: LiveSessionsSnapshot): LiveUsageProvider[] {
  return [
    ...new Set(
      state.sweep.flatMap((count) =>
        count.models.flatMap((model) => {
          const provider = displayedProvider(model.providerRoute)
          return provider && model.working > 0 ? [provider] : []
        }),
      ),
    ),
  ].sort()
}

/** Model vendors do not override recorded provider routes. */
export function liveModels(state: LiveSessionsSnapshot): ProviderModels {
  const models = new Map<LiveUsageProvider, Set<string>>()
  for (const count of state.sweep) {
    for (const model of count.models) {
      const provider = displayedProvider(model.providerRoute)
      if (!provider || model.working <= 0) continue
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
