import { spawn, spawnSync } from "node:child_process"

const args = process.argv.slice(2)
const command = process.platform === "win32" ? "pnpm.cmd" : "pnpm"
const cwd = new URL("..", import.meta.url)
const typecheck = spawnSync(
  command,
  ["exec", "tsc", "--project", "tests/visual/tsconfig.json"],
  {
    cwd,
    stdio: "inherit",
  },
)

if (typecheck.status !== 0) process.exit(typecheck.status ?? 1)

const child = spawn(
  command,
  ["exec", "playwright", "test", "--config", "playwright.config.ts", ...args],
  {
    cwd,
    stdio: "inherit",
  },
)

child.on("exit", (code, signal) => {
  if (signal) process.kill(process.pid, signal)
  else process.exitCode = code ?? 1
})
