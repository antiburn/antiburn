import { expectTypeOf, it } from "vitest"
import type { AppSearchTarget } from "./appSearch"

it("rejects invalid view and Settings destinations at compile time", () => {
  expectTypeOf<{
    kind: "view"
    section: "quota"
    filters: { agents: []; result: "all"; spend: "all" }
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
    filters: { agents: ["claude", "codex"]; result: "failing"; spend: "notable" }
  }>().toExtend<AppSearchTarget>()
  expectTypeOf<{ kind: "setting"; control: "sound" }>().toExtend<AppSearchTarget>()
  expectTypeOf<{ kind: "setting"; pane: "notifications" }>().toExtend<AppSearchTarget>()
})
