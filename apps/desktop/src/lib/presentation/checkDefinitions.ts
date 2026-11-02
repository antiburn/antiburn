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
} as const satisfies Record<
  BurnCheckDetectorId,
  {
    label: string
    aliases: readonly string[]
    kind: "local"
    description: string
  }
>

export const CHECK_LABELS = Object.fromEntries(
  Object.entries(CHECK_DEFINITIONS).map(([id, definition]) => [id, definition.label]),
) as Record<BurnCheckDetectorId, string>

export function isCheckAvailable(
  id: BurnCheckDetectorId,
  _decisionConnectionAvailable: boolean,
): boolean {
  return CHECK_DEFINITIONS[id].kind === "local"
}
