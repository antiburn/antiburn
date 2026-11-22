import assert from "node:assert/strict"
import { execFile, spawn } from "node:child_process"
import { createHash } from "node:crypto"
import {
  chmod,
  copyFile,
  mkdir,
  mkdtemp,
  readFile,
  realpath,
  rm,
  writeFile,
} from "node:fs/promises"
import { tmpdir } from "node:os"
import { delimiter, join } from "node:path"
import { test } from "node:test"
import { fileURLToPath } from "node:url"
import { promisify } from "node:util"

const runner = fileURLToPath(new URL("./run-tauri.mjs", import.meta.url))
const execute = promisify(execFile)

test("all four development launchers use the separate debug configuration", async () => {
  const [release, debug, { scripts }] = await Promise.all(
    [
      "../src-tauri/tauri.conf.json",
      "../src-tauri/tauri.debug.conf.json",
      "../package.json",
    ].map(async (path) => JSON.parse(await readFile(new URL(path, import.meta.url), "utf8"))),
  )
  assert.equal(release.identifier, "ai.antiburn.desktop")
  assert.equal(debug.identifier, "ai.antiburn.desktop.debug")
  for (const name of ["dev", "dev:bundle", "dev:cli", "dev:bundle:cli"]) {
    const args = scripts[name].split(" ")
    assert.ok(args.includes("--with-cli"), `${name} must prepare the managed CLI payload`)
    assert.ok(
      args.some(
        (argument, index) =>
          argument === "--config" && args[index + 1] === "src-tauri/tauri.debug.conf.json",
      ),
      `${name} must load the debug configuration`,
    )
  }
})

test("debug launchers select Cloud without changing release or caller options", async (t) => {
  const directory = await mkdtemp(join(tmpdir(), "antiburn-tauri-args-"))
  const fixture = join(directory, "arguments.mjs")
  const executable = join(directory, process.platform === "win32" ? "tauri.cmd" : "tauri")
  await writeFile(fixture, "process.stdout.write(JSON.stringify(process.argv.slice(2)))\n")
  await writeFile(
    executable,
    process.platform === "win32"
      ? `@"${process.execPath}" "${fixture}" %*\r\n`
      : `#!/bin/sh\nexec "${process.execPath}" "${fixture}" "$@"\n`,
  )
  await chmod(executable, 0o755)
  const { scripts } = JSON.parse(
    await readFile(new URL("../package.json", import.meta.url), "utf8"),
  )
  const dev = scripts.dev
    .split(" ")
    .slice(2)
    .filter((argument) => argument !== "--with-cli")
  const bundle = scripts["dev:bundle"]
    .split(" ")
    .slice(2)
    .filter((argument) => argument !== "--with-cli")
  const cases = [
    { name: "standard dev", input: dev, cloud: true },
    { name: "standard debug bundle", input: bundle, cloud: true },
    { name: "short debug flag", input: ["build", "-d"], cloud: true },
    {
      name: "existing feature lists",
      input: ["dev", "--features", "analytics", "memory-probe", "--features=cloud"],
      cloud: true,
    },
    {
      name: "short feature option",
      input: ["dev", "-f", "analytics,memory-probe"],
      cloud: true,
    },
    {
      name: "target and config options",
      input: ["dev", "--target", "aarch64-apple-darwin", "--config", "debug config.json"],
      cloud: true,
    },
    {
      name: "runner and app arguments",
      input: ["dev", "--", "--verbose", "--", "--example"],
      cloud: true,
    },
    {
      name: "app profile argument",
      input: ["dev", "--", "--", "--profile", "example"],
      cloud: true,
    },
    {
      name: "app options and caller features",
      input: [
        "dev",
        "--features=analytics",
        "--",
        "--",
        "--release",
        "-r",
        "--profile=example",
      ],
      cloud: true,
    },
    { name: "Tauri runner short option", input: ["dev", "-r", "cargo"], cloud: true },
    { name: "release build", input: ["build"], cloud: false },
    { name: "release dev", input: ["dev", "--release"], cloud: false },
    { name: "release runner", input: ["dev", "--", "--release"], cloud: false },
    { name: "short release runner", input: ["dev", "--", "-r"], cloud: false },
    { name: "quiet release cluster", input: ["dev", "--", "-qr"], cloud: false },
    { name: "verbose release cluster", input: ["dev", "--", "-rvv"], cloud: false },
    { name: "mixed release cluster", input: ["dev", "--", "-vvrv"], cloud: false },
    {
      name: "release clusters in app arguments",
      input: ["dev", "--", "--", "-qr", "-rvv"],
      cloud: true,
    },
    { name: "attached feature named r", input: ["dev", "--", "-Fr"], cloud: true },
    {
      name: "separate feature value containing r",
      input: ["dev", "--", "-F", "feature_r"],
      cloud: true,
    },
    {
      name: "cluster ending in an attached feature value",
      input: ["dev", "--", "-qFfeature_r"],
      cloud: true,
    },
    {
      name: "cluster ending in a separate feature value",
      input: ["dev", "--", "-vvF", "feature_r"],
      cloud: true,
    },
    {
      name: "cluster ending in a package value",
      input: ["dev", "--", "-qprunner"],
      cloud: true,
    },
    {
      name: "release before an attached feature value",
      input: ["dev", "--", "-qrFfeature"],
      cloud: false,
    },
    {
      name: "release cluster after a feature value",
      input: ["dev", "--", "-F", "feature_r", "-rvv"],
      cloud: false,
    },
    {
      name: "short release runner with caller features",
      input: ["dev", "--features=analytics", "--", "-r", "--", "--profile", "example"],
      cloud: false,
    },
    {
      name: "short release debug bundle",
      input: ["build", "--debug", "--", "-r"],
      cloud: false,
    },
    { name: "custom profile", input: ["dev", "--", "--profile", "release"], cloud: false },
    {
      name: "debug command with a custom profile",
      input: ["build", "--debug", "--", "--profile=release"],
      cloud: false,
    },
    { name: "debug app argument", input: ["build", "--", "--debug"], cloud: false },
    {
      name: "explicit release features",
      input: ["build", "--features", "analytics,distribution"],
      cloud: false,
    },
    { name: "manual release Cloud", input: ["build", "--features=cloud"], cloud: false },
    { name: "other command", input: ["info"], cloud: false },
    { name: "no command", input: [], cloud: false },
  ]
  try {
    await Promise.all(
      cases.map(({ name, input, cloud }) =>
        t.test(name, async () => {
          const { stdout } = await execute(process.execPath, [runner, ...input], {
            env: { ...process.env, PATH: `${directory}${delimiter}${process.env.PATH ?? ""}` },
            timeout: 10_000,
          })
          assert.deepEqual(
            JSON.parse(stdout),
            cloud ? [input[0], "--features", "cloud", ...input.slice(1)] : input,
          )
        }),
      ),
    )
  } finally {
    await rm(directory, { recursive: true, force: true })
  }
})

test(
  "Desktop stages the Cloud debug CLI while standalone packaging stages the release CLI",
  {
    skip: process.platform === "win32",
  },
  async (t) => {
    const root = await mkdtemp(join(await realpath(tmpdir()), "antiburn-tauri-sidecar-"))
    t.after(() => rm(root, { recursive: true, force: true }))
    const target = "aarch64-apple-darwin"
    const desktop = join(root, "apps", "desktop")
    const packager = join(root, "scripts", "build-cli-sidecar.mjs")
    const launcher = join(desktop, "scripts", "run-tauri.mjs")
    await Promise.all(
      ["scripts", "apps/desktop/scripts", "apps/cli", "bin"].map((path) =>
        mkdir(join(root, path), { recursive: true }),
      ),
    )
    await Promise.all([
      copyFile(runner, launcher),
      copyFile(
        new URL("./cli-dev.mjs", import.meta.url),
        join(desktop, "scripts", "cli-dev.mjs"),
      ),
      copyFile(new URL("../../../scripts/build-cli-sidecar.mjs", import.meta.url), packager),
      writeFile(join(root, "apps", "cli", "Cargo.toml"), '[package]\nversion = "0.1.0"\n'),
    ])
    const cargo = join(root, "cargo.mjs")
    await writeFile(
      cargo,
      `import { mkdirSync, writeFileSync } from "node:fs"
import { join } from "node:path"
const args = process.argv.slice(2)
const target = args[args.indexOf("--target") + 1]
const profile = args.includes("--release") ? "release" : "debug"
const directory = join(process.env.CARGO_TARGET_DIR, target, profile)
mkdirSync(directory, { recursive: true })
const bytes = Buffer.alloc(32)
bytes.writeUInt32LE(0xfeedfacf)
bytes.writeUInt32LE(0x100000c, 4)
bytes.write(profile, 8)
writeFileSync(join(directory, "antiburn"), bytes)
writeFileSync("cargo-args.json", JSON.stringify(args))
`,
    )
    const desktopArgs = join(root, "tauri.mjs")
    await writeFile(
      desktopArgs,
      "process.stdout.write(JSON.stringify(process.argv.slice(2)))\n",
    )
    for (const [name, body] of Object.entries({
      cargo: `exec "${process.execPath}" "${cargo}" "$@"`,
      rustc: `printf 'host: ${target}\\n'`,
      otool: "printf 'binary:\\n /usr/lib/libSystem.B.dylib (compatibility version 1.0.0)\\n'",
      tauri: `exec "${process.execPath}" "${desktopArgs}" "$@"`,
    })) {
      await writeFile(join(root, "bin", name), `#!/bin/sh\n${body}\n`, { mode: 0o755 })
    }
    const env = {
      ...process.env,
      PATH: `${join(root, "bin")}${delimiter}${process.env.PATH ?? ""}`,
    }
    const payload = join(desktop, "src-tauri", "cli-payload")
    const run = (script, args) =>
      execute(process.execPath, [script, ...args], { cwd: root, env })

    const { stdout } = await run(launcher, ["dev", "--with-cli", "--target", target])
    assert.deepEqual(JSON.parse(stdout), [
      "dev",
      "--features",
      "cloud",
      "--target",
      target,
      "--config",
      "src-tauri/tauri.sidecar.conf.json",
    ])
    await checkPayload("debug", ["--features", "cloud"])

    await run(packager, ["--target", target])
    await checkPayload("release", ["--release"])

    async function checkPayload(profile, buildOptions) {
      const cargoArgs = JSON.parse(await readFile(join(root, "cargo-args.json"), "utf8"))
      assert.deepEqual(cargoArgs, [
        "build",
        "--locked",
        ...buildOptions,
        "--manifest-path",
        join(root, "apps", "cli", "Cargo.toml"),
        "--target",
        target,
        "--bin",
        "antiburn",
      ])
      const bytes = await readFile(join(payload, "antiburn-cli"))
      assert.equal(bytes.toString("ascii", 8, 8 + profile.length), profile)
      const manifest = JSON.parse(await readFile(join(payload, "manifest.json"), "utf8"))
      assert.deepEqual(manifest, {
        schema: 1,
        target,
        executable: "antiburn-cli",
        version: "0.1.0",
        sha256: createHash("sha256").update(bytes).digest("hex"),
        delivery_protocols: ["delivery-v1"],
      })
    }
  },
)

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
  "an unexpected Tauri exit stops its process group",
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
trap 'exit 0' TERM
printf '%s %s' "$$" "$grandchild" > "$PID_FILE"
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
      process.kill(childPid, "SIGTERM")
      await waitFor(
        () => Promise.resolve([processExists(wrapper.pid), processExists(grandchildPid)]),
        (alive) => alive.every((value) => !value),
        10_000,
      )
    } finally {
      if (wrapper.exitCode == null && wrapper.signalCode == null) wrapper.kill("SIGKILL")
      await rm(directory, { recursive: true, force: true })
    }
  },
)
