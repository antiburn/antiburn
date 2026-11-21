import { build } from "vite"
import { readFile, writeFile } from "node:fs/promises"
await build({ configFile: "vite.onboarding-demo.config.ts" })
const policy = (await readFile("src/onboarding/demo/csp.txt", "utf8")).trim()
await writeFile(
  "public/onboarding-demo/index.html",
  `<!doctype html><html><head><meta charset="utf-8"><meta http-equiv="Content-Security-Policy" content="${policy}"><link rel="stylesheet" href="./demo.css"></head><body><div id="root"></div><script src="./demo.js"></script></body></html>`,
)
