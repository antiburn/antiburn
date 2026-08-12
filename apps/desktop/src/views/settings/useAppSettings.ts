// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { useCallback, useEffect, useState } from 'react';

import { applyTheme } from '../../lib/appearance';
import { DEFAULT_SETTINGS, getSettings, setSettings, type AppSettings } from '../../lib/ipc';

/**
 * The settings window's copy of the reader's preferences.
 *
 * Writes go straight through — a settings pane with a Save button is a settings
 * pane that can be left in a lying state. The store returns what it actually
 * stored (clamped), and that is what the panes then render, so a value the
 * store refused can never linger on screen.
 *
 * The theme is applied as soon as it is known, and again on every change, so
 * this window follows the choice being made in it.
 */
export interface AppSettingsController {
  settings: AppSettings;
  /** False until the first read resolves. */
  loaded: boolean;
  /** Merge a change into the stored preferences. */
  update: (change: Partial<AppSettings>) => Promise<void>;
}

export function useAppSettings(): AppSettingsController {
  const [settings, setLocal] = useState<AppSettings>(DEFAULT_SETTINGS);
  const [loaded, setLoaded] = useState(false);

  useEffect(() => {
    let active = true;
    getSettings()
      .then((stored) => {
        if (!active) return;
        applyTheme(stored.theme);
        setLocal(stored);
        setLoaded(true);
      })
      .catch(() => {
        if (active) setLoaded(true);
      });
    return () => {
      active = false;
    };
  }, []);

  const update = useCallback(
    async (change: Partial<AppSettings>) => {
      // Optimistic, so a switch does not lag behind the pointer; the stored
      // answer replaces it a moment later and wins any disagreement.
      const next = { ...settings, ...change };
      setLocal(next);
      applyTheme(next.theme);
      const saved = await setSettings(next).catch(() => next);
      setLocal(saved);
      applyTheme(saved.theme);
    },
    [settings],
  );

  return { settings, loaded, update };
}
