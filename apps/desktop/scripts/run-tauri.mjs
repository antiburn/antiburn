import { spawn } from "node:child_process"
import { readFile } from "node:fs/promises"
import { parseEnv } from "node:util"
import { fileURLToPath } from "node:url"

const envPath = fileURLToPath(new URL("../.env", import.meta.url))
let localEnv = {}

try {
  localEnv = parseEnv(await readFile(envPath, "utf8"))
} catch (error) {
  if (error?.code !== "ENOENT") throw error
}

const command = process.platform === "win32" ? "tauri.cmd" : "tauri"
const child = spawn(command, process.argv.slice(2), {
  env: { ...localEnv, ...process.env },
  stdio: "inherit",
})

child.on("error", (error) => {
  console.error(error.message)
  process.exitCode = 1
})

child.on("exit", (code, signal) => {
  if (signal) process.kill(process.pid, signal)
  else process.exitCode = code ?? 1
})
