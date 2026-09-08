#!/usr/bin/env node

import { createReadStream } from "node:fs";
import { resolve } from "node:path";
import { createInterface } from "node:readline";
import { pathToFileURL } from "node:url";

const KINDS = ["cold_launch", "first_open", "warm_reopen"];
const REQUIRED_WARM_SAMPLES = 30;
const WARM_TARGET_MS = 100;

export function collectSample(samples, line) {
  if (!line.trim()) return;
  const { fields } = JSON.parse(line);
  if (fields?.event !== "main_window_revealed") return;
  if (fields.window !== "main") throw new Error("unexpected benchmark window");
  if (!KINDS.includes(fields.open_kind)) throw new Error("unknown open kind");
  if (
    typeof fields.elapsed_ms !== "number" ||
    !Number.isFinite(fields.elapsed_ms) ||
    fields.elapsed_ms < 0
  ) {
    throw new Error("invalid reveal duration");
  }
  if (samples.length >= 100_000)
    throw new Error("select a smaller benchmark log interval");
  samples.push({ kind: fields.open_kind, milliseconds: fields.elapsed_ms });
}

function distribution(values) {
  values.sort((left, right) => left - right);
  const count = values.length;
  if (!count) return { count: 0, median_ms: null, p95_ms: null, max_ms: null };
  const middle = Math.floor(count / 2);
  const median =
    count % 2 ? values[middle] : (values[middle - 1] + values[middle]) / 2;
  return {
    count,
    median_ms: median,
    p95_ms: values[Math.ceil(count * 0.95) - 1],
    max_ms: values[count - 1],
  };
}

export function summarize(samples) {
  const groups = Object.fromEntries(
    KINDS.map((kind) => [
      kind,
      distribution(
        samples
          .filter((sample) => sample.kind === kind)
          .map((sample) => sample.milliseconds),
      ),
    ]),
  );
  const warm = groups.warm_reopen;
  return {
    metric: "request_to_native_reveal_ms",
    presented_frame_measured: false,
    groups,
    warm_native_target: {
      required_samples: REQUIRED_WARM_SAMPLES,
      p95_limit_ms: WARM_TARGET_MS,
      status:
        warm.count < REQUIRED_WARM_SAMPLES
          ? "insufficient_samples"
          : warm.p95_ms <= WARM_TARGET_MS
            ? "met"
            : "missed",
    },
  };
}

async function main(args) {
  if (args.length !== 1 || args[0] === "--help") {
    process.stdout.write(
      "Usage: node scripts/window-open-report.mjs <hourly-json-log|->\nUse one platform, build, and benchmark interval. '-' reads standard input.\n",
    );
    if (args[0] !== "--help") process.exitCode = 1;
    return;
  }
  const input =
    args[0] === "-"
      ? process.stdin
      : createReadStream(args[0], { encoding: "utf8" });
  const lines = createInterface({ input, crlfDelay: Infinity });
  const samples = [];
  try {
    for await (const line of lines) collectSample(samples, line);
  } finally {
    lines.close();
    if (input !== process.stdin) input.destroy();
  }
  if (!samples.length) throw new Error("no main-window reveal samples found");
  process.stdout.write(`${JSON.stringify(summarize(samples), null, 2)}\n`);
}

if (
  process.argv[1] &&
  import.meta.url === pathToFileURL(resolve(process.argv[1])).href
) {
  main(process.argv.slice(2)).catch((error) => {
    process.stderr.write(`${error.message}\n`);
    process.exitCode = 1;
  });
}
