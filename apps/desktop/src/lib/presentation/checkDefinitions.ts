import type { BurnCheckDetectorId } from "../insightsIpc"

export const CHECK_DEFINITIONS = {
  sessionsOverDepth: {
    label: "Session overdepth",
    aliases: ["long sessions", "context depth", "conversation length"],
    kind: "local",
    description: "Checks recorded context depth against documented limits.",
  },
  modelOverthinking: {
    label: "Model overthinking",
    aliases: ["reasoning effort", "thinking budget"],
    kind: "local",
    description: "Checks recorded reasoning settings and usage against documented limits.",
  },
  overpoweredSubagents: {
    label: "Overpowered subagents",
    aliases: ["expensive subagents", "delegation", "model routing"],
    kind: "local",
    description: "Checks whether helper models use more power than their tasks need.",
  },
  unusedMcpServers: {
    label: "Unused MCP servers",
    aliases: ["MCP tools", "unused servers"],
    kind: "local",
    description: "Checks whether connected tool servers load tools that sessions do not use.",
  },
  unusedBuiltInTools: {
    label: "Unused built-in tools",
    aliases: ["built in tools", "unused tool definitions"],
    kind: "local",
    description: "Checks recorded optional built-in tool definitions and calls.",
  },
  unusedSkills: {
    label: "Unused skills",
    aliases: ["skill instructions", "unused skills"],
    kind: "local",
    description: "Checks recorded skill injection and use.",
  },
  oldModelUsage: {
    label: "Old model usage",
    aliases: ["outdated models", "model versions"],
    kind: "local",
    description: "Checks recorded model versions against documented replacements.",
  },
  skillOpportunities: {
    label: "Skill opportunities",
    aliases: ["improve skills", "skill improvements", "skill suggestions"],
    kind: "smart",
    description: "Finds a current skill procedure that could help a selected workflow.",
  },
  overExploring: {
    label: "Over-exploring",
    aliases: ["over_exploring", "redundant reads", "repeated reads", "unrelated files"],
    kind: "smart",
    description:
      "Checks selected file reads for redundant output or a clearly separate purpose.",
  },
  scopeCreep: {
    label: "Scope creep",
    aliases: ["scope_creep", "extra work", "agreed task", "task scope", "unapproved work"],
    kind: "smart",
    description: "Checks selected performed work for a substantial unapproved objective.",
  },
  overuseOfFastMode: {
    label: "Fast mode overuse",
    aliases: ["fast mode", "priority", "speed"],
    kind: "local",
    description: "Checks recorded fast-tier use against documented limits.",
  },
  cacheChurn: {
    label: "Excess cache rehydration",
    aliases: ["prompt cache", "rehydration", "cache misses"],
    kind: "local",
    description: "Checks compatible request records for excess cache rehydration.",
  },
  ignoredInstructions: {
    label: "Ignored instructions",
    aliases: ["instruction conflicts", "missed agent rules", "AGENTS.md", "CLAUDE.md"],
    kind: "smart",
    description: "Checks selected actions for direct conflicts with current instruction rules.",
  },
} as const satisfies Record<
  BurnCheckDetectorId,
  {
    label: string
    aliases: readonly string[]
    kind: "local" | "smart"
    description: string
  }
>

export const CHECK_LABELS = Object.fromEntries(
  Object.entries(CHECK_DEFINITIONS).map(([id, definition]) => [id, definition.label]),
) as Record<BurnCheckDetectorId, string>

export function isCheckAvailable(
  id: BurnCheckDetectorId,
  smartChecksAvailable: boolean,
): boolean {
  return smartChecksAvailable || CHECK_DEFINITIONS[id].kind === "local"
}

const OVER_EXPLORING_DETAILS = {
  unrelated_files: "A selected read sample suggests a purpose separate from the task.",
  excessive_file_breadth: "The work read more files than it needed.",
  excessive_within_file_reading:
    "Later reads returned identical recorded output with no observed intervening work.",
} as const

export function overExploringDetail(reason: string | undefined): string | null {
  switch (reason) {
    case "unrelated_files":
    case "excessive_file_breadth":
    case "excessive_within_file_reading":
      return OVER_EXPLORING_DETAILS[reason]
    default:
      return null
  }
}
