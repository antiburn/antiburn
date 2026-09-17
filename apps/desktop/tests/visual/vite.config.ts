import { fileURLToPath, URL } from "node:url"

import tailwindcss from "@tailwindcss/vite"
import babel from "@rolldown/plugin-babel"
import react, { reactCompilerPreset } from "@vitejs/plugin-react"
import { defineConfig } from "vite"

const fixture = (path: string) => fileURLToPath(new URL(path, import.meta.url))

export default defineConfig({
  plugins: [react(), babel({ presets: [reactCompilerPreset()] }), tailwindcss()],
  resolve: {
    alias: {
      "@tauri-apps/api/core": fixture("./fakes/core.ts"),
      "@tauri-apps/api/event": fixture("./fakes/event.ts"),
      "@tauri-apps/api/window": fixture("./fakes/window.ts"),
      "@tauri-apps/api/dpi": fixture("./fakes/dpi.ts"),
      "@tauri-apps/api/webviewWindow": fixture("./fakes/webviewWindow.ts"),
      "@tauri-apps/plugin-dialog": fixture("./fakes/dialog.ts"),
      "@tauri-apps/plugin-clipboard-manager": fixture("./fakes/clipboard.ts"),
    },
  },
  server: { host: "127.0.0.1", port: 4174, strictPort: true },
})
