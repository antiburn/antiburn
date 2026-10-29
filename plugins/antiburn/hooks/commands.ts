import { describe } from "../lib/product.mjs"

export const ARGUMENT_HINT = "[refresh | help]"
export const COMMAND = "antiburn"
export const describeCommands = () => describe("claude-code", `/${COMMAND}`, "native")
