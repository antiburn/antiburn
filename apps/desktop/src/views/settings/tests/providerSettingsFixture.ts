import type { ProviderSettings } from "../../../lib/smartCheckProviders"

export function providerSettingsFixture(
  input: Pick<ProviderSettings, "activeId" | "profiles"> &
    Partial<Pick<ProviderSettings, "pendingOperation" | "displayNumbers">>,
): ProviderSettings {
  const displayNumbers = { ...input.displayNumbers }
  const counters = new Map<string, number>()
  for (const [id, profile] of Object.entries(input.profiles))
    counters.set(
      profile.provider,
      Math.max(counters.get(profile.provider) ?? 0, displayNumbers[id] ?? 0),
    )
  for (const [id, profile] of Object.entries(input.profiles)) {
    if (displayNumbers[id] !== undefined) continue
    const number = (counters.get(profile.provider) ?? 0) + 1
    displayNumbers[id] = number
    counters.set(profile.provider, number)
  }
  return { ...input, displayNumbers, pendingOperation: input.pendingOperation ?? null }
}
