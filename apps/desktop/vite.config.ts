import { fileURLToPath, URL } from "node:url"

import tailwindcss from "@tailwindcss/vite"
import babel from "@rolldown/plugin-babel"
import react, { reactCompilerPreset } from "@vitejs/plugin-react"
// `vitest/config` re-exports Vite's `defineConfig` with the `test` block typed.
import { defineConfig } from "vitest/config"

/** Port the Tauri shell expects for the dev server (see `tauri.conf.json`). */
const DEV_SERVER_PORT = 1420

export default defineConfig(({ command, mode }) => ({
  plugins: [
    {
      name: "tauri-webview-module-cache",
      configureServer(server) {
        server.middlewares.use((request, _response, next) => {
          // A new Tauri webview can lack the cached body for a conditional Vite response. Send every module body.
          delete request.headers["if-none-match"]
          delete request.headers["if-modified-since"]
          next()
        })
      },
    },
    react(),
    babel({ presets: [reactCompilerPreset()] }),
    tailwindcss(),
  ],

  // Tauri reads the shell's stderr, so Vite must not clear it, and the dev
  // server has to stay on the exact port `devUrl` points at.
  clearScreen: false,
  server: {
    host: "127.0.0.1",
    port: DEV_SERVER_PORT,
    strictPort: true,
    // Do not store a partial module graph for a later webview.
    headers: { "Cache-Control": "no-store" },
  },

  // Only variables with these prefixes reach the renderer.
  envPrefix: ["VITE_", "TAURI_ENV_"],

  build: {
    rollupOptions: {
      input: {
        main: fileURLToPath(new URL("./index.html", import.meta.url)),
        onboarding: fileURLToPath(new URL("./onboarding.html", import.meta.url)),
        settings: fileURLToPath(new URL("./settings.html", import.meta.url)),
      },
    },
    // The shell is the only consumer, so target the bundled webviews rather
    // than the browser matrix Vite defaults to.
    target: process.env.TAURI_ENV_PLATFORM === "windows" ? "chrome110" : "safari15",
    // Source maps are useful for Vite's dev server and Tauri debug bundles,
    // but shipping them alongside every embedded webview multiplies the
    // frontend payload without helping an installed reader. Tauri exposes
    // `TAURI_ENV_DEBUG` to its build hook; the mode check keeps direct
    // `vite --mode development` builds equally debuggable.
    sourcemap:
      command === "serve" || mode !== "production" || process.env.TAURI_ENV_DEBUG === "true",
    emptyOutDir: true,
  },

  test: {
    environment: "jsdom",
    // The label helpers format wall-clock times. Keep their output equal on all CI hosts.
    env: { TZ: "UTC" },
    setupFiles: ["./src/test/setup.ts"],
    include: ["src/**/*.{test,spec}.{ts,tsx}", "tests/**/*.{test,spec}.ts"],
  },
}))
