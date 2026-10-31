import assert from "node:assert/strict"
import { test } from "node:test"
import { prepareCliDev } from "./cli-dev.mjs"

for (const host of [
  "aarch64-apple-darwin",
  "x86_64-pc-windows-msvc",
  "aarch64-unknown-linux-gnu",
]) {
  test(`explicit CLI setup builds and maps the host payload: ${host}`, () => {
    const built = []
    const input = ["dev", "--with-cli", "--config", "src-tauri/tauri.debug.conf.json"]
    const output = prepareCliDev(input, {
      readHost: () => `host: ${host}\n`,
      buildPayload: (target) => built.push(target),
    })
    assert.deepEqual(built, [host])
    assert.deepEqual(output, [
      "dev",
      "--config",
      "src-tauri/tauri.debug.conf.json",
      "--config",
      "src-tauri/tauri.sidecar.conf.json",
    ])
    assert.ok(input.includes("--with-cli"))
  })
}

test("ordinary launch commands do not read the host or build payloads", () => {
  const args = ["dev", "--config", "src-tauri/tauri.debug.conf.json"]
  const fail = () => {
    throw new Error("unexpected preparation")
  }
  assert.equal(prepareCliDev(args, { readHost: fail, buildPayload: fail }), args)
})

test("debug bundles are prepared and build failures prevent launching", () => {
  const args = ["build", "--debug", "--with-cli"]
  assert.throws(
    () =>
      prepareCliDev(args, {
        readHost: () => "host: aarch64-apple-darwin",
        buildPayload: () => {
          throw new Error("build failed")
        },
      }),
    /build failed/,
  )
})

test("unsupported and cross-target development requests fail before building", () => {
  for (const args of [
    ["dev", "--with-cli", "--target=x86_64-pc-windows-msvc"],
    ["build", "--with-cli"],
  ]) {
    assert.throws(() =>
      prepareCliDev(args, {
        readHost: () => "host: aarch64-apple-darwin",
        buildPayload: () => {
          assert.fail("must not build")
        },
      }),
    )
  }
  assert.throws(
    () =>
      prepareCliDev(["dev", "--with-cli"], {
        readHost: () => "host: unsupported",
        buildPayload: () => {
          assert.fail("must not build")
        },
      }),
    /Unsupported/,
  )
})

test("missing or empty target values fail before building", () => {
  for (const suffix of [["--target"], ["--target="], ["--target", "--config", "debug.json"]]) {
    assert.throws(
      () =>
        prepareCliDev(["dev", "--with-cli", ...suffix], {
          readHost: () => "host: aarch64-apple-darwin",
          buildPayload: () => {
            assert.fail("must not build")
          },
        }),
      /requires a value/,
    )
  }
})
