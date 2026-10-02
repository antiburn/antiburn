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

/** Short, plain problem phrases for a failing category. The Overview's
 *  first-run summary lists these instead of the category label, so the
 *  reader sees what is wrong, not just its name. */
export const CHECK_PROBLEM_PHRASES: Record<BurnCheckDetectorId, string> = {
  sessionsOverDepth: "sessions run long before compacting",
  modelOverthinking: "thinking level is higher than the work needs",
  overpoweredSubagents: "subagents run on the main model",
  unusedMcpServers: "MCP servers are loaded but never called",
  unusedBuiltInTools: "built-in tools are loaded but never used",
  unusedSkills: "skills are injected but never used",
  oldModelUsage: "sessions use old model versions",
  overuseOfFastMode: "fast mode runs where it doesn't pay off",
  cacheChurn: "cache is rehydrated more than it needs to be",
  ignoredInstructions: "agents go against your AGENTS.md or CLAUDE.md rules",
}
