// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

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
    passedTitle: "No session overdepth detected",
    failedTitle: "Session overdepth detected",
  },
  {
    id: "modelOverthinking",
    passedTitle: "No model overthinking detected",
    failedTitle: "Model overthinking detected",
  },
  {
    id: "overpoweredSubagents",
    passedTitle: "No overpowered subagents detected",
    failedTitle: "Overpowered subagents detected",
  },
  {
    id: "obsoleteModel",
    passedTitle: "No obsolete model detected",
    failedTitle: "Obsolete model detected",
  },
  {
    id: "fastModeOveruse",
    passedTitle: "No fast mode overuse detected",
    failedTitle: "Fast mode overuse detected",
  },
  {
    id: "excessCacheRehydration",
    passedTitle: "No excess cache rehydration detected",
    failedTitle: "Excess cache rehydration detected",
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
