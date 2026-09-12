/** The compact label omits these known provider wrappers. */
const MODEL_PREFIXES = [
  "antigravity-claude-",
  "anthropic/claude-",
  "anthropic.claude-",
  "openai/gpt-",
  "openai.gpt-",
  "claude-",
  "gpt-",
] as const

export interface PresentableModelRun {
  model: string
  thinkingMode?: string | undefined
}

/** Return a compact label for a model ID. Keep unknown IDs unchanged. */
export function modelShortName(model: string): string {
  const trimmed = model.trim()
  const prefix = MODEL_PREFIXES.find((candidate) => trimmed.startsWith(candidate))
  return prefix ? trimmed.slice(prefix.length) : trimmed
}

/** Format one model run. */
export function modelRunName(run: PresentableModelRun): string {
  const model = run.model.trim()
  const mode = run.thinkingMode?.trim()
  return model && mode ? `${model}/${mode}` : model
}

/** Format model runs without changing their order. Remove duplicate labels. */
export function modelRunNames(runs: readonly PresentableModelRun[]): string[] {
  const seen = new Set<string>()
  const names: string[] = []
  for (const run of runs) {
    const name = modelRunName(run)
    if (!name || seen.has(name)) continue
    seen.add(name)
    names.push(name)
  }
  return names
}

/** Shorten the model IDs in formatted model runs. */
export function modelRunShortNames(runs: readonly PresentableModelRun[]): string[] {
  return modelRunNames(runs.map((run) => ({ ...run, model: modelShortName(run.model) })))
}

/**
 * Shorten model runs and keep the name and the thinking mode apart, for a
 * caller that styles the two differently. Order and duplicate removal
 * match modelRunShortNames.
 */
export function modelRunShortPairs(
  runs: readonly PresentableModelRun[],
): { model: string; thinkingMode?: string | undefined }[] {
  const seen = new Set<string>()
  const pairs: { model: string; thinkingMode?: string | undefined }[] = []
  for (const run of runs) {
    const shortened = { ...run, model: modelShortName(run.model) }
    const name = modelRunName(shortened)
    if (!name || seen.has(name)) continue
    seen.add(name)
    const mode = shortened.thinkingMode?.trim()
    pairs.push({ model: shortened.model.trim(), ...(mode ? { thinkingMode: mode } : {}) })
  }
  return pairs
}

/**
 * Tokens that name a vendor, not a model. A scope name and a model id
 * disagree on these: the provider scopes a window to "Fable" while the
 * session states `claude-fable-5`.
 */
const GENERIC_MODEL_TOKENS = new Set([
  "anthropic",
  "claude",
  "codex",
  "gemini",
  "google",
  "gpt",
  "openai",
])

/** The lower-case word parts of a model name or id. */
function modelTokens(value: string): string[] {
  return value
    .toLowerCase()
    .split(/[^a-z0-9]+/)
    .filter((token) => token.length > 0)
}

/**
 * True when `modelId` names the model a usage window is scoped to.
 *
 * A provider scopes a window with a display name, such as "Fable", and a
 * session states a raw id, such as `claude-fable-5`. The comparison drops
 * the vendor tokens from the scope name and keeps the version, so "Fable"
 * does not match `claude-opus-4-6` and "Sonnet 4.5" does not match
 * `claude-sonnet-3-7`. A name the id spells differently matches nothing,
 * and the meter then stays still rather than claim a model is running.
 */
export function modelMatchesScope(modelId: string, scopeName: string): boolean {
  const wanted = modelTokens(scopeName).filter((token) => !GENERIC_MODEL_TOKENS.has(token))
  if (wanted.length === 0) return false
  const found = new Set(modelTokens(modelId))
  return wanted.every((token) => found.has(token))
}
