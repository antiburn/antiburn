type MainViewDefinition = {
  id: string
  label: string
  aliases: readonly string[]
}

export const MAIN_VIEWS = [
  { id: "agents", label: "Agents", aliases: ["coding agents", "discovered agents"] },
  { id: "quota", label: "Limits", aliases: ["quota", "provider limits", "usage limits"] },
  { id: "burnChecks", label: "Checks", aliases: ["burn checks", "checks", "waste", "savings"] },
  {
    id: "memories",
    label: "Memories",
    aliases: ["memory", "auto-memory", "agent memory", "claude memory"],
  },
  { id: "activity", label: "Sessions", aliases: ["activity", "history"] },
] as const satisfies readonly MainViewDefinition[]

/** Accept the retired ID only at compatibility boundaries. */
export type MainViewId = (typeof MAIN_VIEWS)[number]["id"] | "overview"

export const SIDEBAR_MAIN_VIEWS = ["activity", "burnChecks"] as const

export function isMainViewId(id: string): id is MainViewId {
  return MAIN_VIEWS.some((view) => view.id === id)
}
