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
