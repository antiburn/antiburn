// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import tailwindcss from '@tailwindcss/vite';
import react from '@vitejs/plugin-react';
// `vitest/config` re-exports Vite's `defineConfig` with the `test` block typed.
import { defineConfig } from 'vitest/config';

/** Port the Tauri shell expects for the dev server (see `tauri.conf.json`). */
const DEV_SERVER_PORT = 1420;

export default defineConfig({
  plugins: [react(), tailwindcss()],

  // Tauri reads the shell's stderr, so Vite must not clear it, and the dev
  // server has to stay on the exact port `devUrl` points at.
  clearScreen: false,
  server: {
    host: '127.0.0.1',
    port: DEV_SERVER_PORT,
    strictPort: true,
  },

  // Only variables with these prefixes reach the renderer.
  envPrefix: ['VITE_', 'TAURI_ENV_'],

  build: {
    // The shell is the only consumer, so target the bundled webviews rather
    // than the browser matrix Vite defaults to.
    target: process.env.TAURI_ENV_PLATFORM === 'windows' ? 'chrome110' : 'safari15',
    sourcemap: true,
    emptyOutDir: true,
  },

  test: {
    environment: 'jsdom',
    setupFiles: ['./src/test/setup.ts'],
    // `tests/` holds checks that must not live inside the tree they check
    // (see tests/no-exfiltration.test.ts).
    include: ['src/**/*.{test,spec}.{ts,tsx}', 'tests/**/*.{test,spec}.ts'],
  },
});
