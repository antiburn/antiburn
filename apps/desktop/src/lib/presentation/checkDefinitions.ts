import type { BurnCheckDetectorId } from "../insightsIpc"

export const CHECK_DEFINITIONS = {
  sessionsOverDepth: {
    label: "Session overdepth",
    aliases: ["long sessions", "context depth", "conversation length"],
  },
  modelOverthinking: {
    label: "Model overthinking",
    aliases: ["reasoning effort", "thinking budget"],
  },
  overpoweredSubagents: {
    label: "Overpowered subagents",
    aliases: ["expensive subagents", "delegation", "model routing"],
  },
  unusedMcpServers: { label: "Unused MCP servers", aliases: ["MCP tools", "unused servers"] },
  unusedBuiltInTools: {
    label: "Unused built-in tools",
    aliases: ["built in tools", "unused tool definitions"],
  },
  unusedSkills: { label: "Unused skills", aliases: ["skill instructions", "unused skills"] },
  skillOpportunities: {
    label: "Skill Opportunities",
    aliases: ["improve skills", "skill improvements", "skill suggestions"],
  },
  overExploring: {
    label: "Over-exploring",
    aliases: ["over_exploring", "excessive reading", "unrelated files", "file breadth"],
  },
  scopeCreep: {
    label: "Scope Creep",
    aliases: ["scope_creep", "extra work", "agreed task", "task scope", "unapproved work"],
  },
  oldModelUsage: { label: "Old model usage", aliases: ["outdated models", "model versions"] },
  overuseOfFastMode: {
    label: "Fast mode overuse",
    aliases: ["fast mode", "priority", "speed"],
  },
  cacheChurn: {
    label: "Excess cache rehydration",
    aliases: ["prompt cache", "rehydration", "cache misses"],
  },
  ignoredInstructions: {
    label: "Ignored Instructions",
    aliases: ["instruction conflicts", "missed agent rules", "AGENTS.md", "CLAUDE.md"],
  },
} as const satisfies Record<BurnCheckDetectorId, { label: string; aliases: readonly string[] }>

export const CHECK_LABELS = Object.fromEntries(
  Object.entries(CHECK_DEFINITIONS).map(([id, definition]) => [id, definition.label]),
) as Record<BurnCheckDetectorId, string>

export function isCheckAvailable(
  id: BurnCheckDetectorId,
  smartChecksAvailable: boolean,
): boolean {
  return (
    smartChecksAvailable ||
    (id !== "ignoredInstructions" &&
      id !== "skillOpportunities" &&
      id !== "overExploring" &&
      id !== "scopeCreep")
  )
}

export const OVER_EXPLORING_DETAILS = {
  unrelated_files: "The assessed reads included files unrelated to the work.",
  excessive_file_breadth: "The assessed work read more files than it needed.",
  excessive_within_file_reading: "The assessed work read more of a file than it needed.",
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
