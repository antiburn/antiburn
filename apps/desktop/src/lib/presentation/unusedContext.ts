import type { SessionHygienePayload, UnusedResource } from "../insightsIpc"
import { SMALL_COST_USD } from "./sessionAnalysis"

export type UnusedContextKind = "MCP server" | "Built-in tool" | "Skill"

/** One idle resource this session carried, ready to render as a row. */
export interface UnusedContextRow {
  name: string
  kind: UnusedContextKind
  costUsd: number | null
}

/**
 * One row of the display list: either a single idle resource, or a rollup
 * that stands in for several small-cost resources of the same kind. See
 * {@link unusedContextDisplayRows}.
 */
export type UnusedContextDisplayRow =
  | { type: "item"; name: string; kind: UnusedContextKind; costUsd: number | null }
  | { type: "rollup"; kind: UnusedContextKind; count: number; names: string[]; costUsd: number }

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

/**
 * Rolls small priced rows of the same kind into one summary row, so a long
 * tail of near-zero items does not crowd out the rows that matter.
 *
 * A row is "small" when it carries a price below {@link SMALL_COST_USD}. A
 * kind with two or more small rows collapses into one rollup row carrying
 * their count, alphabetically sorted names, and summed cost; a kind with
 * only one small row keeps it as an item. Unpriced rows are never rolled up.
 * Item rows keep their incoming order (cost descending, unpriced last);
 * rollup rows follow, sorted by summed cost descending, ties broken by kind.
 */
export function unusedContextDisplayRows(rows: UnusedContextRow[]): UnusedContextDisplayRow[] {
  const smallByKind = new Map<UnusedContextKind, UnusedContextRow[]>()
  for (const row of rows) {
    if (row.costUsd == null || row.costUsd >= SMALL_COST_USD) continue
    const list = smallByKind.get(row.kind)
    if (list) list.push(row)
    else smallByKind.set(row.kind, [row])
  }

  const rolledKinds = new Set(
    [...smallByKind.entries()].filter(([, list]) => list.length >= 2).map(([kind]) => kind),
  )

  const items: UnusedContextDisplayRow[] = rows
    .filter(
      (row) =>
        !rolledKinds.has(row.kind) || row.costUsd == null || row.costUsd >= SMALL_COST_USD,
    )
    .map((row) => ({ type: "item", name: row.name, kind: row.kind, costUsd: row.costUsd }))

  const rollups: UnusedContextDisplayRow[] = [...rolledKinds]
    .map((kind) => {
      const list = smallByKind.get(kind) ?? []
      const names = list.map((row) => row.name).sort((a, b) => a.localeCompare(b))
      const costUsd = list.reduce((sum, row) => sum + (row.costUsd ?? 0), 0)
      return { type: "rollup" as const, kind, count: list.length, names, costUsd }
    })
    .sort((a, b) => {
      if (a.costUsd !== b.costUsd) return b.costUsd - a.costUsd
      return a.kind.localeCompare(b.kind)
    })

  return [...items, ...rollups]
}
