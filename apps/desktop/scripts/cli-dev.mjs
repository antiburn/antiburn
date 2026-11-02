import { execFileSync } from "node:child_process"
import { build, targets } from "../../../scripts/build-cli-sidecar.mjs"

export function prepareCliDev(
  args,
  {
    readHost = () => execFileSync("rustc", ["-vV"], { encoding: "utf8" }),
    buildPayload = build,
  } = {},
) {
  if (!args.includes("--with-cli")) return args
  const host = /^host: (.+)$/m.exec(readHost())?.[1]?.trim()
  if (!targets.includes(host)) throw new Error("Unsupported CLI development host")
  const targetIndex = args.indexOf("--target")
  const target =
    targetIndex >= 0
      ? args[targetIndex + 1]
      : args.find((arg) => arg.startsWith("--target="))?.slice(9)
  const hasTarget = targetIndex >= 0 || args.some((arg) => arg.startsWith("--target="))
  if (hasTarget && (!target || target.startsWith("-")))
    throw new Error("CLI development target requires a value")
  if (target && target !== host)
    throw new Error("CLI development setup requires the host target")
  if (args[0] !== "dev" && !(args[0] === "build" && args.includes("--debug")))
    throw new Error("CLI development setup requires dev or build --debug")
  buildPayload(host)
  return [
    ...args.filter((arg) => arg !== "--with-cli"),
    "--config",
    "src-tauri/tauri.sidecar.conf.json",
  ]
}
