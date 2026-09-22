import { describe, expect, it } from "vitest"
import { MAIN_VIEWS, isMainViewId } from "./mainViews"

describe("main view registry", () => {
  it("has unique stable IDs and nonempty labels", () => {
    expect(new Set(MAIN_VIEWS.map(({ id }) => id)).size).toBe(MAIN_VIEWS.length)
    for (const view of MAIN_VIEWS) {
      expect(view.id.trim()).not.toBe("")
      expect(view.label.trim()).not.toBe("")
      expect(isMainViewId(view.id)).toBe(true)
    }
  })

  it.each(["", "unknown", "toString", "constructor", "__proto__", "filter:all"])(
    "rejects non-view IDs: %s",
    (id) => expect(isMainViewId(id)).toBe(false),
  )
})
