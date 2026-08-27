import { fileURLToPath } from "node:url";

import { runAislopHook } from "./aislop-hook-output.mjs";

export async function run(stdinText, bin) {
  return runAislopHook(stdinText, bin, "claude aislop hook");
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  let stdin = "";
  process.stdin.setEncoding("utf8");
  for await (const chunk of process.stdin) stdin += chunk;
  process.exitCode = await run(stdin);
}
