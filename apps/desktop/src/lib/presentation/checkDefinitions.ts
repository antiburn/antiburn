import type { BurnCheckDetectorId } from "../insightsIpc"

export const CHECK_DEFINITIONS = {
  sessionsOverDepth: {
    label: "Session overdepth",
    aliases: ["long sessions", "context depth", "conversation length"],
    kind: "local",
    description: "Checks recorded context depth against reviewed limits.",
  },
  modelOverthinking: {
    label: "Model overthinking",
    aliases: ["reasoning effort", "thinking budget"],
    kind: "local",
    description: "Checks recorded reasoning settings and usage against reviewed limits.",
  },
  overpoweredSubagents: {
    label: "Overpowered subagents",
    aliases: ["expensive subagents", "delegation", "model routing"],
    kind: "local",
    description: "Checks recorded parent and worker model combinations.",
  },
  unusedMcpServers: {
    label: "Unused MCP servers",
    aliases: ["MCP tools", "unused servers"],
    kind: "local",
    description: "Checks recorded MCP server exposure and calls.",
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
    description: "Checks recorded model versions against reviewed replacements.",
  },
  overuseOfFastMode: {
    label: "Fast mode overuse",
    aliases: ["fast mode", "priority", "speed"],
    kind: "local",
    description: "Checks recorded fast-tier use against reviewed limits.",
  },
  cacheChurn: {
    label: "Excess cache rehydration",
    aliases: ["prompt cache", "rehydration", "cache misses"],
    kind: "local",
    description: "Checks compatible request records for excess cache rehydration.",
  },
  ignoredInstructions: {
    label: "Ignored Instructions",
    aliases: ["instruction conflicts", "missed agent rules", "AGENTS.md", "CLAUDE.md"],
    kind: "smart",
    description:
      "Finds project instructions a session did not follow. Checks start after 3 minutes of inactivity.",
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

/** How many selected checks can currently run. Smart checks also need their
 * provider gate to be active. A snoozed check still runs, so it counts. */
export function enabledCheckCount(
  checks: readonly { id: BurnCheckDetectorId; enabled: boolean }[],
  smartChecksConfigured: boolean,
): number {
  return checks.filter(({ id, enabled }) => {
    const definition = CHECK_DEFINITIONS[id]
    return (
      enabled && Boolean(definition) && (definition.kind === "local" || smartChecksConfigured)
    )
  }).length
}

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
