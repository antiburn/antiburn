import { execFileSync } from "node:child_process"
import { build, targets } from "../../../scripts/build-cli-sidecar.mjs"

/** Stage the host payload when requested and add its Tauri resource config. */
export function prepareCliDev(
  args,
  {
    cloudDev = false,
    readHost = () => execFileSync("rustc", ["-vV"], { encoding: "utf8" }),
    buildPayload = build,
  } = {},
) {
  const separator = args.indexOf("--")
  const tauriArgs = separator < 0 ? args : args.slice(0, separator)
  const forwardedArgs = separator < 0 ? [] : args.slice(separator)
  if (!tauriArgs.includes("--with-cli")) return args
  const host = /^host: (.+)$/m.exec(readHost())?.[1]?.trim()
  if (!targets.includes(host)) throw new Error("Unsupported CLI development host")
  const targetIndex = tauriArgs.indexOf("--target")
  const target =
    targetIndex >= 0
      ? tauriArgs[targetIndex + 1]
      : tauriArgs.find((arg) => arg.startsWith("--target="))?.slice(9)
  const hasTarget = targetIndex >= 0 || tauriArgs.some((arg) => arg.startsWith("--target="))
  if (hasTarget && (!target || target.startsWith("-")))
    throw new Error("CLI development target requires a value")
  if (target && target !== host)
    throw new Error("CLI development setup requires the host target")
  const debug = tauriArgs.includes("--debug") || tauriArgs.includes("-d")
  if (tauriArgs[0] !== "dev" && !(tauriArgs[0] === "build" && debug))
    throw new Error("CLI development setup requires dev or build --debug")
  buildPayload(host, { cloudDev })
  return [
    ...tauriArgs.filter((arg) => arg !== "--with-cli"),
    "--config",
    "src-tauri/tauri.sidecar.conf.json",
    ...forwardedArgs,
  ]
}
