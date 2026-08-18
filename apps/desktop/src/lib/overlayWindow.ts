// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { invoke } from '@tauri-apps/api/core';
import { WebviewWindow } from '@tauri-apps/api/webviewWindow';

/** Must match `OVERLAY_LABEL` in `src-tauri/src/overlay_window.rs`. */
export const OVERLAY_WINDOW_LABEL = 'antiburn-overlay';

/** Open (or re-show) the always-on-top usage overlay. */
export function openOverlayWindow(): Promise<void> {
  return invoke('open_overlay_window');
}

/** Hide the overlay if it exists (no-op when it was never opened). */
export async function hideOverlayWindow(): Promise<void> {
  const overlay = await WebviewWindow.getByLabel(OVERLAY_WINDOW_LABEL);
  await overlay?.hide();
}

/* "Show floating session HUD" preference. All antiburn windows share one
   origin, so localStorage is a common store: Settings writes it, the popover
   reads it at startup to restore the HUD, and the HUD's ✕ clears it so the
   toggle stays truthful. */
const HUD_PREF_KEY = 'antiburn.showFloatingHud';

export function isFloatingHudEnabled(): boolean {
  try {
    return localStorage.getItem(HUD_PREF_KEY) === '1';
  } catch {
    return false;
  }
}

export function setFloatingHudEnabled(enabled: boolean): void {
  try {
    localStorage.setItem(HUD_PREF_KEY, enabled ? '1' : '0');
  } catch {
    // Preference is best-effort; the HUD still opens/hides for this run.
  }
}

/** Whether the overlay is on screen right now. Asked of the window itself
 * rather than the stored preference: the HUD's own ✕ can hide it while the
 * popover is closed, and each antiburn webview holds its own localStorage
 * copy, so the preference drifts. */
export async function isOverlayWindowVisible(): Promise<boolean> {
  try {
    const overlay = await WebviewWindow.getByLabel(OVERLAY_WINDOW_LABEL);
    return (await overlay?.isVisible()) ?? false;
  } catch {
    return false;
  }
}
