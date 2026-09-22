import { spawn } from "node:child_process"
import { chmod, mkdtemp, readFile, rm, writeFile } from "node:fs/promises"
import { tmpdir } from "node:os"
import { delimiter, join } from "node:path"
import { test } from "node:test"
import { fileURLToPath } from "node:url"

const runner = fileURLToPath(new URL("./run-tauri.mjs", import.meta.url))

async function waitFor(read, accept, timeoutMs = 5_000) {
  const deadline = Date.now() + timeoutMs
  while (Date.now() < deadline) {
    const value = await read().catch(() => null)
    if (value != null && accept(value)) return value
    await new Promise((resolve) => setTimeout(resolve, 20))
  }
  throw new Error("timed out waiting for the process state")
}

function processExists(pid) {
  try {
    process.kill(pid, 0)
    return true
  } catch (error) {
    if (error?.code === "ESRCH") return false
    throw error
  }
}

test(
  "a termination signal stops the Tauri process group",
  { skip: process.platform === "win32" },
  async () => {
    const directory = await mkdtemp(join(tmpdir(), "antiburn-tauri-runner-"))
    const executable = join(directory, "tauri")
    const pidFile = join(directory, "pids")
    await writeFile(
      executable,
      `#!/bin/sh
sleep 300 &
grandchild=$!
printf '%s %s' "$$" "$grandchild" > "$PID_FILE"
wait
`,
    )
    await chmod(executable, 0o755)

    const wrapper = spawn(process.execPath, [runner], {
      env: {
        ...process.env,
        PATH: `${directory}${delimiter}${process.env.PATH ?? ""}`,
        PID_FILE: pidFile,
      },
      stdio: "ignore",
    })

    try {
      const [childPid, grandchildPid] = await waitFor(
        () => readFile(pidFile, "utf8"),
        (value) => value.trim().split(" ").length === 2,
      ).then((value) => value.trim().split(" ").map(Number))
      wrapper.kill("SIGTERM")
      await waitFor(
        () => Promise.resolve([processExists(childPid), processExists(grandchildPid)]),
        (alive) => alive.every((value) => !value),
      )
    } finally {
      if (wrapper.exitCode == null && wrapper.signalCode == null) wrapper.kill("SIGKILL")
      await rm(directory, { recursive: true, force: true })
    }
  },
)

test(
  "an unexpected Tauri exit stops tracked descendants",
  { skip: process.platform === "win32" },
  async () => {
    const directory = await mkdtemp(join(tmpdir(), "antiburn-tauri-runner-"))
    const executable = join(directory, "tauri")
    const pidFile = join(directory, "pids")
    await writeFile(
      executable,
      `#!/bin/sh
sh -c 'trap "" TERM; sleep 300' &
grandchild=$!
printf '%s %s' "$$" "$grandchild" > "$PID_FILE"
trap 'exit 0' TERM
wait "$grandchild"
`,
    )
    await chmod(executable, 0o755)

    const wrapper = spawn(process.execPath, [runner], {
      env: {
        ...process.env,
        PATH: `${directory}${delimiter}${process.env.PATH ?? ""}`,
        PID_FILE: pidFile,
      },
      stdio: "ignore",
    })

    try {
      const [childPid, grandchildPid] = await waitFor(
        () => readFile(pidFile, "utf8"),
        (value) => value.trim().split(" ").length === 2,
      ).then((value) => value.trim().split(" ").map(Number))
      await new Promise((resolve) => setTimeout(resolve, 150))
      process.kill(childPid, "SIGTERM")
      await waitFor(
        () => Promise.resolve([processExists(wrapper.pid), processExists(grandchildPid)]),
        (alive) => alive.every((value) => !value),
      )
    } finally {
      if (wrapper.exitCode == null && wrapper.signalCode == null) wrapper.kill("SIGKILL")
      await rm(directory, { recursive: true, force: true })
    }
  },
)
