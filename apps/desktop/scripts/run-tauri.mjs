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

const isWindows = process.platform === "win32"
const command = isWindows ? "tauri.cmd" : "tauri"
// Node refuses to spawn a `.cmd` file directly since 20.x, so Windows goes
// through the shell. The shell receives one joined command line, which splits
// an argument that contains a space, so quote those arguments here.
const args = process.argv.slice(2).map((argument) => {
  if (!isWindows || !/[\s&|<>^"]/.test(argument)) return argument
  return `"${argument.replaceAll('"', '\\"')}"`
})
const child = spawn(command, args, {
  env: { ...localEnv, ...process.env },
  stdio: "inherit",
  shell: isWindows,
})

child.on("error", (error) => {
  console.error(error.message)
  process.exitCode = 1
})

child.on("exit", (code, signal) => {
  if (signal) process.kill(process.pid, signal)
  else process.exitCode = code ?? 1
})
