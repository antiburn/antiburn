// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { useEffect, useState } from 'react';

import { engineCatalogVersion, openSettingsWindow } from '../lib/shell';

/**
 * Placeholder contents of the tray popover.
 *
 * Activity, session analytics, and repository surfaces land in later streams;
 * this view exists to prove the window, the IPC path, and the engine link.
 */
export function PopoverView() {
  const catalogVersion = useEngineCatalogVersion();

  return (
    <div className="flex h-full flex-col">
      <header className="flex items-center justify-between border-b border-black/10 px-4 py-3 dark:border-white/10">
        <h1 className="text-[13px] font-semibold tracking-tight">antiburn</h1>
        <button
          type="button"
          className="rounded px-2 py-1 text-[12px] text-neutral-600 hover:bg-black/5 dark:text-neutral-300 dark:hover:bg-white/10"
          onClick={() => {
            void openSettingsWindow();
          }}
        >
          Settings
        </button>
      </header>

      <main className="flex flex-1 items-center justify-center px-6 text-center">
        <p className="text-[12px] leading-relaxed text-neutral-500 dark:text-neutral-400">
          Local session activity will appear here.
        </p>
      </main>

      <footer className="border-t border-black/10 px-4 py-2 text-[11px] text-neutral-500 dark:border-white/10 dark:text-neutral-400">
        Pricing catalog {catalogVersion ?? '—'}
      </footer>
    </div>
  );
}

/**
 * Reads the engine's bundled pricing-catalog version once per mount.
 *
 * Resolves to `null` outside the shell and on failure, which keeps the footer
 * rendering a placeholder instead of surfacing an error to a skeleton view.
 */
function useEngineCatalogVersion(): string | null {
  const [version, setVersion] = useState<string | null>(null);

  useEffect(() => {
    let active = true;
    engineCatalogVersion()
      .then((value) => {
        if (active) {
          setVersion(value);
        }
      })
      .catch(() => {
        if (active) {
          setVersion(null);
        }
      });
    return () => {
      active = false;
    };
  }, []);

  return version;
}
