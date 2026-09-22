import { spawn, spawnSync } from "node:child_process"
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
  detached: !isWindows,
})

let shutdownSignal = null
let forceKillTimer = null

function processGroupExists() {
  if (!child.pid) return false
  try {
    process.kill(-child.pid, 0)
    return true
  } catch (error) {
    if (error?.code === "ESRCH") return false
    throw error
  }
}

function signalChild(signal) {
  if (!child.pid || (isWindows && (child.exitCode != null || child.signalCode != null))) return
  if (isWindows) {
    const args = ["/pid", String(child.pid), "/t"]
    if (signal === "SIGKILL") args.push("/f")
    spawnSync("taskkill", args, { stdio: "ignore" })
    return
  }
  try {
    process.kill(-child.pid, signal)
  } catch (error) {
    if (error?.code !== "ESRCH") throw error
  }
}

function scheduleForceKill() {
  if (forceKillTimer) clearTimeout(forceKillTimer)
  forceKillTimer = setTimeout(() => {
    forceKillTimer = null
    signalChild("SIGKILL")
  }, 5_000)
}

function finishChildExit(code, signal) {
  const exitSignal = shutdownSignal ?? signal
  if (exitSignal) process.kill(process.pid, exitSignal)
  else process.exitCode = code ?? 1
}

function cleanupAfterExit(code, signal) {
  if (isWindows) {
    finishChildExit(code, signal)
    return
  }

  signalChild("SIGTERM")
  setTimeout(() => {
    if (!processGroupExists()) {
      finishChildExit(code, signal)
      return
    }
    forceKillTimer = setTimeout(() => {
      forceKillTimer = null
      signalChild("SIGKILL")
      finishChildExit(code, signal)
    }, 5_000)
  }, 100)
}

for (const signal of ["SIGINT", "SIGTERM", "SIGHUP"]) {
  process.once(signal, () => {
    shutdownSignal = signal
    signalChild(signal)
    scheduleForceKill()
  })
}

child.on("error", (error) => {
  console.error(error.message)
  process.exitCode = 1
})

child.on("exit", (code, signal) => {
  if (forceKillTimer) clearTimeout(forceKillTimer)
  cleanupAfterExit(code, signal)
})

process.on("exit", () => signalChild("SIGTERM"))
