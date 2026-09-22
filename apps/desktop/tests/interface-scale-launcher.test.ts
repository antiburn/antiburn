import { EventEmitter } from "node:events"
import { createRequire } from "node:module"
import path from "node:path"
import { fileURLToPath, pathToFileURL } from "node:url"

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

const mocks = vi.hoisted(() => ({ spawn: vi.fn(), spawnSync: vi.fn() }))
vi.mock("node:child_process", () => ({ ...mocks, default: mocks }))

const launcher = pathToFileURL(
  path.resolve(
    path.dirname(fileURLToPath(import.meta.url)),
    "../scripts/test-interface-scale.mjs",
  ),
)
const require = createRequire(launcher)
const compilerPackage = require.resolve("@typescript/native/package.json")
const compiler = path.resolve(path.dirname(compilerPackage), require(compilerPackage).bin.tsc)
const playwright = require.resolve("@playwright/test/cli")
const originalArgv = process.argv
const originalExitCode = process.exitCode
let child: EventEmitter

beforeEach(() => {
  vi.resetModules()
  mocks.spawnSync.mockReset().mockReturnValue({ status: 0 })
  child = new EventEmitter()
  mocks.spawn.mockReset().mockReturnValue(child)
  vi.spyOn(process, "exit").mockImplementation((code) => {
    throw new Error(`exit:${code}`)
  })
  vi.spyOn(process, "kill").mockReturnValue(true)
  vi.spyOn(console, "error").mockImplementation(() => {})
  process.argv = [process.execPath, fileURLToPath(launcher)]
  process.exitCode = undefined
})

afterEach(() => {
  process.argv = originalArgv
  process.exitCode = originalExitCode
  vi.restoreAllMocks()
})

const run = () => import(/* @vite-ignore */ launcher.href)

describe("interface-scale visual test launcher", () => {
  it("runs installed JS entry points through Node without a platform shell", async () => {
    await run()
    const options = {
      cwd: path.resolve(path.dirname(fileURLToPath(launcher)), ".."),
      stdio: "inherit",
    }
    expect(mocks.spawnSync).toHaveBeenCalledExactlyOnceWith(
      process.execPath,
      [compiler, "--project", "tests/visual/tsconfig.json"],
      options,
    )
    expect(mocks.spawn).toHaveBeenCalledExactlyOnceWith(
      process.execPath,
      [playwright, "test", "--config", "playwright.config.ts"],
      options,
    )
  })

  it("preserves spaces, quotes and shell metacharacters as literal arguments", async () => {
    const args = [
      "--grep",
      "scale & focus | (90|200)%",
      "--output",
      'C:\\QA Results\\a "quote"',
    ]
    process.argv.push(...args)
    await run()
    expect(mocks.spawn.mock.calls[0]?.[1]).toEqual([
      playwright,
      "test",
      "--config",
      "playwright.config.ts",
      ...args,
    ])
    expect(mocks.spawn.mock.calls[0]?.[2].shell).toBeUndefined()
  })

  it("does not launch Playwright if type checking fails", async () => {
    mocks.spawnSync.mockReturnValue({ status: 2 })
    await expect(run()).rejects.toThrow("exit:2")
    expect(mocks.spawn).not.toHaveBeenCalled()
  })

  it("reports a failed compiler launch instead of silently exiting", async () => {
    mocks.spawnSync.mockReturnValue({ status: null, error: new Error("cannot spawn Node") })
    await expect(run()).rejects.toThrow("exit:1")
    expect(console.error).toHaveBeenCalledWith("cannot spawn Node")
    expect(mocks.spawn).not.toHaveBeenCalled()
  })

  it("reports a failed Playwright launch", async () => {
    await run()
    child.emit("error", new Error("cannot spawn Playwright"))
    expect(console.error).toHaveBeenCalledWith("cannot spawn Playwright")
    expect(process.exitCode).toBe(1)
  })

  it.each([0, 3])("preserves Playwright exit code %s", async (code) => {
    await run()
    child.emit("exit", code, null)
    expect(process.exitCode).toBe(code)
  })

  it("preserves child termination by signal", async () => {
    await run()
    child.emit("exit", null, "SIGTERM")
    expect(process.kill).toHaveBeenCalledWith(process.pid, "SIGTERM")
  })
})
