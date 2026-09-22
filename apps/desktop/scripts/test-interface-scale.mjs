import { spawn, spawnSync } from "node:child_process"
import { createRequire } from "node:module"
import { dirname, resolve } from "node:path"
import { fileURLToPath } from "node:url"

const require = createRequire(import.meta.url)
const compilerPackage = require.resolve("@typescript/native/package.json")
const compiler = resolve(dirname(compilerPackage), require(compilerPackage).bin.tsc)
const playwright = require.resolve("@playwright/test/cli")
const args = process.argv.slice(2)
const cwd = resolve(dirname(fileURLToPath(import.meta.url)), "..")
const typecheck = spawnSync(
  process.execPath,
  [compiler, "--project", "tests/visual/tsconfig.json"],
  {
    cwd,
    stdio: "inherit",
  },
)

if (typecheck.error) console.error(typecheck.error.message)
if (typecheck.status !== 0) process.exit(typecheck.status ?? 1)

const child = spawn(
  process.execPath,
  [playwright, "test", "--config", "playwright.config.ts", ...args],
  {
    cwd,
    stdio: "inherit",
  },
)

child.on("error", (error) => {
  console.error(error.message)
  process.exitCode = 1
})

child.on("exit", (code, signal) => {
  if (signal) process.kill(process.pid, signal)
  else process.exitCode = code ?? 1
})
