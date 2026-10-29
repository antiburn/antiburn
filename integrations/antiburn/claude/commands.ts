import { describe } from "../../../packages/antiburn-plugin-core/product.mjs"

export const ARGUMENT_HINT = "[refresh | help]"
export const COMMAND = "antiburn"
export const describeCommands = () => describe("claude-code", `/${COMMAND}`, "native")
