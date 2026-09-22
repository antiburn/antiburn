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
})

let shutdownSignal = null
let forceKillTimer = null
let trackedDescendants = []

function readDescendants(rootPid) {
  const result = spawnSync("ps", ["-axo", "pid=,ppid="], { encoding: "utf8" })
  const children = new Map()
  for (const line of result.stdout?.split("\n") ?? []) {
    const [pid, parentPid] = line.trim().split(/\s+/).map(Number)
    if (!Number.isInteger(pid) || !Number.isInteger(parentPid)) continue
    const siblings = children.get(parentPid) ?? []
    siblings.push(pid)
    children.set(parentPid, siblings)
  }
  const descendants = []
  const pending = [...(children.get(rootPid) ?? [])]
  while (pending.length > 0) {
    const pid = pending.pop()
    descendants.push(pid)
    pending.push(...(children.get(pid) ?? []))
  }
  return descendants.reverse()
}

function signalProcesses(pids, signal) {
  for (const pid of pids) {
    try {
      process.kill(pid, signal)
    } catch (error) {
      if (error?.code !== "ESRCH") throw error
    }
  }
}

function signalChild(signal) {
  if (!child.pid || child.exitCode != null || child.signalCode != null) return
  if (isWindows) {
    const args = ["/pid", String(child.pid), "/t"]
    if (signal === "SIGKILL") args.push("/f")
    spawnSync("taskkill", args, { stdio: "ignore" })
    return
  }
  trackedDescendants = readDescendants(child.pid)
  signalProcesses([...trackedDescendants, child.pid], signal)
}

const descendantTimer = isWindows
  ? null
  : setInterval(() => {
      if (child.pid && child.exitCode == null && child.signalCode == null) {
        trackedDescendants = readDescendants(child.pid)
      }
    }, 100)
descendantTimer?.unref()

for (const signal of ["SIGINT", "SIGTERM", "SIGHUP"]) {
  process.once(signal, () => {
    shutdownSignal = signal
    signalChild(signal)
    forceKillTimer = setTimeout(() => signalChild("SIGKILL"), 5_000)
    forceKillTimer.unref()
  })
}

child.on("error", (error) => {
  console.error(error.message)
  process.exitCode = 1
})

child.on("exit", (code, signal) => {
  if (forceKillTimer) clearTimeout(forceKillTimer)
  if (descendantTimer) clearInterval(descendantTimer)
  if (!isWindows) {
    signalProcesses(trackedDescendants, "SIGTERM")
    signalProcesses(trackedDescendants, "SIGKILL")
  }
  const exitSignal = shutdownSignal ?? signal
  if (exitSignal) process.kill(process.pid, exitSignal)
  else process.exitCode = code ?? 1
})

process.on("exit", () => signalChild("SIGTERM"))
