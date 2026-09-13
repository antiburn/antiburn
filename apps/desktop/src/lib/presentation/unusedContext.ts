import type { SessionHygienePayload, UnusedResource } from "../insightsIpc"

type UnusedContextKind = "MCP server" | "Built-in tool" | "Skill"

/** One idle resource this session carried, ready to render as a row. */
export interface UnusedContextRow {
  name: string
  kind: UnusedContextKind
  costUsd: number | null
}

function rowsOfKind(resources: UnusedResource[], kind: UnusedContextKind): UnusedContextRow[] {
  return resources.map((resource) => ({ name: resource.name, kind, costUsd: resource.costUsd }))
}

/**
 * Every resource this session's hygiene evidence found injected but never
 * called, across all three kinds. Sorts by cost, highest first; a row with
 * no resolved price sorts last; ties break by name. Returns an empty list
 * when the payload carries no `unusedResources` evidence.
 */
export function unusedContextRows(hygiene: SessionHygienePayload): UnusedContextRow[] {
  const unused = hygiene.unusedResources
  if (!unused) return []
  const rows = [
    ...rowsOfKind(unused.mcpServers, "MCP server"),
    ...rowsOfKind(unused.builtInTools, "Built-in tool"),
    ...rowsOfKind(unused.skills, "Skill"),
  ]
  return rows.sort((a, b) => {
    if (a.costUsd == null && b.costUsd == null) return a.name.localeCompare(b.name)
    if (a.costUsd == null) return 1
    if (b.costUsd == null) return -1
    if (a.costUsd !== b.costUsd) return b.costUsd - a.costUsd
    return a.name.localeCompare(b.name)
  })
}

/** Sum of every priced row's cost, or null when no row carries a price. */
export function unusedContextTotalUsd(rows: UnusedContextRow[]): number | null {
  const priced = rows.filter(
    (row): row is UnusedContextRow & { costUsd: number } => row.costUsd != null,
  )
  if (priced.length === 0) return null
  return priced.reduce((total, row) => total + row.costUsd, 0)
}
