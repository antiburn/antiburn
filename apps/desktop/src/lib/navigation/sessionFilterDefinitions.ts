export const FIXED_SESSION_FILTERS = [
  { id: "notable", label: "Notable Sessions", group: "featured" },
  { id: "material", label: "Material Sessions", group: "featured" },
  { id: "failing", label: "Failing Sessions", group: "status" },
  { id: "passing", label: "Passing Sessions", group: "status" },
  { id: "all", label: "All Sessions", group: "all" },
] as const

type FixedSessionFilterId = (typeof FIXED_SESSION_FILTERS)[number]["id"]
export type SessionFilter = { kind: FixedSessionFilterId } | { kind: "agent"; agent: string }
