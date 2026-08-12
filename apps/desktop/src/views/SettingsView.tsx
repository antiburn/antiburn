// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { Info, Palette, RefreshCw, SlidersHorizontal, FolderGit2 } from 'lucide-react';
import { useEffect, useState } from 'react';

import { ScrollPane } from '../components/ui/ScrollPane';
import { SidebarNav, type SidebarNavItem } from '../components/ui/SidebarNav';
import { appInfo, type AppInfo } from '../lib/ipc';
import { AboutPane } from './settings/AboutPane';
import { AppearancePane } from './settings/AppearancePane';
import { GeneralPane } from './settings/GeneralPane';
import { SourcesPane } from './settings/SourcesPane';
import { UpdatesPane } from './settings/UpdatesPane';
import { useAppSettings } from './settings/useAppSettings';

/**
 * The standalone settings window: a source list on the left, one pane on the
 * right.
 *
 * Every pane writes through immediately (see `useAppSettings`), so the window
 * has no Save button and no dirty state — closing it can never discard a
 * choice.
 */

const PANES: readonly SidebarNavItem[] = [
  { id: 'general', label: 'General', icon: SlidersHorizontal },
  { id: 'appearance', label: 'Appearance', icon: Palette },
  { id: 'sources', label: 'Sources', icon: FolderGit2 },
  { id: 'updates', label: 'Updates', icon: RefreshCw, separatorBefore: true },
  { id: 'about', label: 'About', icon: Info },
];

export function SettingsView() {
  const [pane, setPane] = useState<string>('general');
  const [info, setInfo] = useState<AppInfo | null>(null);
  const controller = useAppSettings();

  useEffect(() => {
    let active = true;
    appInfo()
      .then((resolved) => {
        if (active) setInfo(resolved);
      })
      .catch(() => {
        if (active) setInfo(null);
      });
    return () => {
      active = false;
    };
  }, []);

  return (
    <div className="flex h-full min-h-0">
      <SidebarNav items={PANES} value={pane} onChange={setPane} ariaLabel="Settings sections" />

      <div className="flex min-h-0 min-w-0 flex-1 flex-col">
        <ScrollPane viewportClassName="px-6 py-5">
          <div
            role="tabpanel"
            id={`${pane}-panel`}
            aria-labelledby={`${pane}-tab`}
            className="mx-auto w-full max-w-[520px]"
          >
            {pane === 'general' && <GeneralPane {...controller} />}
            {pane === 'appearance' && <AppearancePane {...controller} />}
            {pane === 'sources' && <SourcesPane />}
            {pane === 'updates' && <UpdatesPane {...controller} info={info} />}
            {pane === 'about' && <AboutPane info={info} />}
          </div>
        </ScrollPane>
      </div>
    </div>
  );
}
