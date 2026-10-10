type MainViewDefinition = {
  id: string
  label: string
  aliases: readonly string[]
}

export const MAIN_VIEWS = [
  { id: "overview", label: "Overview", aliases: ["home", "dashboard", "usage", "cost"] },
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

export type MainViewId = (typeof MAIN_VIEWS)[number]["id"]

export function isMainViewId(id: string): id is MainViewId {
  return MAIN_VIEWS.some((view) => view.id === id)
}
