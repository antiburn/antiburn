export const FIXED_SESSION_FILTERS = [
  { id: "notable", label: "Notable Sessions", aliases: ["filter"], group: "featured" },
  { id: "material", label: "Material Sessions", aliases: ["filter"], group: "featured" },
  { id: "failing", label: "Failing Sessions", aliases: ["filter"], group: "status" },
  { id: "passing", label: "Passing Sessions", aliases: ["filter"], group: "status" },
  { id: "all", label: "All Sessions", aliases: [], group: "all" },
] as const

export type FixedSessionFilterId = (typeof FIXED_SESSION_FILTERS)[number]["id"]
export type SessionFilter = { kind: FixedSessionFilterId } | { kind: "agent"; agent: string }
