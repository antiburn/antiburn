// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { invoke, isTauri } from '@tauri-apps/api/core';

/**
 * Thin, typed wrappers over the shell's commands.
 *
 * The bundle also has to load in a plain browser (`pnpm dev:web`, unit tests),
 * where no shell is attached. Every wrapper therefore reports absence rather
 * than throwing, so views can render a degraded state instead of crashing.
 */

/** True when the bundle is running inside the antiburn shell. */
export function hasShell(): boolean {
  return isTauri();
}

/**
 * Version stamp of the engine's bundled pricing catalog.
 *
 * This is the end-to-end proof that the shell links the local engine: the
 * value originates in `antiburn_local::pricing::PRICING_CATALOG_VERSION`.
 * Returns `null` when no shell is attached.
 */
export async function engineCatalogVersion(): Promise<string | null> {
  if (!hasShell()) {
    return null;
  }
  return invoke<string>('engine_catalog_version');
}

/** Opens (or refocuses) the standalone settings window. */
export async function openSettingsWindow(): Promise<void> {
  if (!hasShell()) {
    return;
  }
  await invoke('open_settings_window');
}
