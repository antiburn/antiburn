// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

/**
 * Placeholder contents of the standalone settings window.
 *
 * Repository selection, scan roots, appearance, and update preferences land in
 * later streams.
 */
export function SettingsView() {
  return (
    <div className="flex h-full flex-col gap-2 px-6 py-5">
      <h1 className="text-[15px] font-semibold tracking-tight">Settings</h1>
      <p className="text-[12px] leading-relaxed text-neutral-500 dark:text-neutral-400">
        Preferences will appear here.
      </p>
    </div>
  );
}
