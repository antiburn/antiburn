import { expectTypeOf, it } from "vitest"
import type { AppSearchTarget } from "./appSearch"

it("rejects invalid view and Settings destinations at compile time", () => {
  expectTypeOf<{
    kind: "view"
    section: "quota"
    filter: { kind: "all" }
  }>().not.toExtend<AppSearchTarget>()
  expectTypeOf<{
    kind: "setting"
    pane: "general"
    control: "sound"
  }>().not.toExtend<AppSearchTarget>()
  expectTypeOf<{ kind: "setting"; control: "unknown" }>().not.toExtend<AppSearchTarget>()
  expectTypeOf<{
    kind: "view"
    section: "activity"
    filter: { kind: "notable" }
  }>().toExtend<AppSearchTarget>()
  expectTypeOf<{ kind: "setting"; control: "sound" }>().toExtend<AppSearchTarget>()
  expectTypeOf<{ kind: "setting"; pane: "notifications" }>().toExtend<AppSearchTarget>()
})
