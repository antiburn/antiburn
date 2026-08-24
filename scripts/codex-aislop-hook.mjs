import { fileURLToPath } from "node:url";

import { AISLOP_BIN, runAislopHook } from "./aislop-hook-output.mjs";

export { AISLOP_BIN };

export function patchFiles(patch) {
  const entries = [];
  let state = null;
  let moveEntry = null;

  for (const line of patch.split("\n")) {
    const isBody =
      (state === "update" &&
        (line === "" || line.startsWith(" ") || line.startsWith("+") || line.startsWith("-"))) ||
      (state === "add" && line.startsWith("+"));

    if (isBody) {
      moveEntry = null;
      continue;
    }

    const header = line.trim();
    const precedingUpdate = moveEntry;
    moveEntry = null;

    if (header.startsWith("*** Add File: ")) {
      entries.push({ path: header.slice("*** Add File: ".length) });
      state = "add";
    } else if (header.startsWith("*** Update File: ")) {
      const entry = { path: header.slice("*** Update File: ".length) };
      entries.push(entry);
      state = "update";
      moveEntry = entry;
    } else if (header.startsWith("*** Delete File: ")) {
      state = null;
    } else if (header.startsWith("*** Move to: ")) {
      const previous = entries.at(-1);
      if (previous && previous === precedingUpdate) {
        previous.path = header.slice("*** Move to: ".length);
      }
    } else if (header === "*** Begin Patch") {
      state = null;
    } else if (header === "*** End Patch") {
      break;
    } else if (header.startsWith("@@")) {
      state = "update";
    } else {
      state = null;
    }
  }

  const seen = new Set();
  return entries
    .map(({ path }) => path)
    .filter((path) => path && !seen.has(path) && seen.add(path));
}

export async function run(stdinText, bin = AISLOP_BIN) {
  if (stdinText.trim() === "") return 0;

  let event;
  try {
    event = JSON.parse(stdinText);
  } catch {
    process.stderr.write("codex aislop hook ignored invalid JSON\n");
    return 0;
  }

  if (event === null || typeof event !== "object" || Array.isArray(event)) {
    process.stderr.write("codex aislop hook ignored a non-object event\n");
    return 0;
  }

  const command = event.tool_input?.command;
  if (typeof command !== "string") return 0;

  const files = patchFiles(command);
  if (files.length === 0) return 0;

  const input = JSON.stringify({
    tool_input: { edits: files.map((file_path) => ({ file_path })) },
  });

  return runAislopHook(input, bin, "codex aislop hook");
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  let stdin = "";
  process.stdin.setEncoding("utf8");
  for await (const chunk of process.stdin) stdin += chunk;
  process.exitCode = await run(stdin);
}
