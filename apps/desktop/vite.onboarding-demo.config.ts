import { fileURLToPath, URL } from "node:url"
import { defineConfig } from "vite"
import react, { reactCompilerPreset } from "@vitejs/plugin-react"
import babel from "@rolldown/plugin-babel"
import tailwindcss from "@tailwindcss/vite"
const root = fileURLToPath(new URL(".", import.meta.url))
const adapter = (name: string) =>
  fileURLToPath(new URL(`./src/onboarding/demo/adapters/${name}.ts`, import.meta.url))
export default defineConfig({
  root,
  publicDir: false,
  define: { "process.env.NODE_ENV": JSON.stringify("production") },
  plugins: [
    react(),
    babel({ presets: [reactCompilerPreset()] }),
    tailwindcss(),
    {
      name: "demo-promotions",
      resolveId(id) {
        if (id === "virtual:promotions") return "\0demo-promotions"
      },
      load(id) {
        if (id === "\0demo-promotions") return "export default []"
      },
    },
  ],
  resolve: {
    alias: Object.fromEntries(
      ["core", "event", "window", "dpi", "webviewWindow"]
        .map((name) => [`@tauri-apps/api/${name}`, adapter(name)])
        .concat([
          ["@tauri-apps/plugin-dialog", adapter("dialog")],
          ["@tauri-apps/plugin-clipboard-manager", adapter("clipboard")],
        ]),
    ),
  },
  build: {
    target: process.env.TAURI_ENV_PLATFORM === "windows" ? "chrome110" : "safari15",
    outDir: "public/onboarding-demo",
    emptyOutDir: true,
    lib: {
      entry: fileURLToPath(new URL("./src/onboarding/demo/main.tsx", import.meta.url)),
      name: "OnboardingDemo",
      formats: ["iife"],
      fileName: () => "demo.js",
      cssFileName: "demo",
    },
    rollupOptions: { output: { inlineDynamicImports: true } },
  },
})
