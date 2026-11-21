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

test("manual launch without CLI preparation does not read the host or build payloads", () => {
  const args = ["dev", "--config", "src-tauri/tauri.debug.conf.json"]
  const fail = () => {
    throw new Error("unexpected preparation")
  }
  assert.equal(prepareCliDev(args, { readHost: fail, buildPayload: fail }), args)
})

test("CLI preparation preserves Cloud features and other Tauri options", () => {
  const input = [
    "dev",
    "--with-cli",
    "--features",
    "cloud",
    "--features=analytics",
    "--target",
    "aarch64-apple-darwin",
    "--config",
    "src-tauri/tauri.debug.conf.json",
  ]
  assert.deepEqual(
    prepareCliDev(input, {
      readHost: () => "host: aarch64-apple-darwin",
      buildPayload: () => {},
    }),
    [
      ...input.filter((argument) => argument !== "--with-cli"),
      "--config",
      "src-tauri/tauri.sidecar.conf.json",
    ],
  )
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

test("sidecar options precede the runner separator and preserve all forwarded arguments", () => {
  const forwarded = ["--", "--", "--verbose", "--target", "other-target", "--with-cli"]
  const built = []
  const args = ["dev", "--with-cli", ...forwarded]
  assert.deepEqual(
    prepareCliDev(args, {
      readHost: () => "host: aarch64-apple-darwin",
      buildPayload: (target) => built.push(target),
    }),
    ["dev", "--config", "src-tauri/tauri.sidecar.conf.json", ...forwarded],
  )
  assert.deepEqual(built, ["aarch64-apple-darwin"])
})

test("forwarded preparation and debug flags do not change Tauri build selection", () => {
  const args = ["dev", "--", "--with-cli", "--target=other-target"]
  const fail = () => assert.fail("forwarded flags must not prepare a payload")
  assert.equal(prepareCliDev(args, { readHost: fail, buildPayload: fail }), args)
  assert.throws(
    () =>
      prepareCliDev(["build", "--with-cli", "--", "--debug"], {
        readHost: () => "host: aarch64-apple-darwin",
        buildPayload: fail,
      }),
    /requires dev or build --debug/,
  )
})
