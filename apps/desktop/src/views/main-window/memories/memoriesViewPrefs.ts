/**
 * The Memories screen's own view state a reader last chose: which project
 * groups are collapsed. Persisted to `localStorage`, the same pattern
 * `quotaViewPrefs.ts` uses, so the screen reopens the way the reader left it.
 */

const MEMORIES_VIEW_PREFS_KEY = "antiburn.memories.view.v1"

/** The saved slugs of collapsed project groups, or an empty list with no
 *  saved value, no storage, or a stored value of the wrong shape. */
export function readCollapsedProjects(): string[] {
  try {
    const raw = localStorage.getItem(MEMORIES_VIEW_PREFS_KEY)
    if (!raw) return []
    const parsed: unknown = JSON.parse(raw)
    if (parsed == null || typeof parsed !== "object" || Array.isArray(parsed)) return []
    const slugs = (parsed as { collapsedProjects?: unknown }).collapsedProjects
    return Array.isArray(slugs)
      ? slugs.filter((slug): slug is string => typeof slug === "string")
      : []
  } catch {
    return []
  }
}

/** Save the slugs of collapsed project groups. */
export function writeCollapsedProjects(slugs: Iterable<string>): void {
  try {
    localStorage.setItem(
      MEMORIES_VIEW_PREFS_KEY,
      JSON.stringify({ collapsedProjects: [...slugs] }),
    )
  } catch {
    // The screen still works when preference storage is unavailable.
  }
}
