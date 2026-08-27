export interface MockSessionHygieneCheck {
  id:
    | "sessionOverdepth"
    | "modelOverthinking"
    | "overpoweredSubagents"
    | "obsoleteModel"
    | "fastModeOveruse"
    | "excessCacheRehydration"
  passed: boolean
  title: string
}

interface HygieneCheckDefinition {
  id: MockSessionHygieneCheck["id"]
  passedTitle: string
  failedTitle: string
}

const CHECKS: readonly HygieneCheckDefinition[] = [
  {
    id: "sessionOverdepth",
    passedTitle: "No session overdepth",
    failedTitle: "Session overdepth",
  },
  {
    id: "modelOverthinking",
    passedTitle: "No model overthinking",
    failedTitle: "Model overthinking",
  },
  {
    id: "overpoweredSubagents",
    passedTitle: "No overpowered subagents",
    failedTitle: "Overpowered subagents",
  },
  {
    id: "obsoleteModel",
    passedTitle: "No obsolete model",
    failedTitle: "Obsolete model",
  },
  {
    id: "fastModeOveruse",
    passedTitle: "No fast mode overuse",
    failedTitle: "Fast mode overuse",
  },
  {
    id: "excessCacheRehydration",
    passedTitle: "No excess cache rehydration",
    failedTitle: "Excess cache rehydration",
  },
]

/** Return a stable unsigned hash for prototype data. */
function stableHash(value: string): number {
  let hash = 2_166_136_261
  for (let index = 0; index < value.length; index += 1) {
    hash ^= value.charCodeAt(index)
    hash = Math.imul(hash, 16_777_619)
  }
  return hash >>> 0
}

/** Build deterministic mock hygiene results for one session. */
export function mockSessionHygiene(seed: string): MockSessionHygieneCheck[] {
  return CHECKS.map((check) => {
    // The modulus sets the mock failure rate. At 24, most sessions pass.
    const passed = stableHash(`${seed}:${check.id}`) % 24 !== 0
    return {
      id: check.id,
      passed,
      title: passed ? check.passedTitle : check.failedTitle,
    }
  })
}
