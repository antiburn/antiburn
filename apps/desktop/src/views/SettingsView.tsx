// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import {
  Bell,
  Info,
  LogOut,
  Palette,
  RefreshCw,
  ShieldCheck,
  SlidersHorizontal,
  FolderGit2,
} from 'lucide-react';
import { useEffect, useState } from 'react';

import { ScrollPane } from '../components/ui/ScrollPane';
import { SidebarNav, type SidebarNavItem } from '../components/ui/SidebarNav';
import {
  appInfo,
  onSettingsPaneRequest,
  quitApp,
  takeSettingsPane,
  type AppInfo,
} from '../lib/ipc';
import { isSettingsPane, type SettingsPane } from '../lib/settingsPanes';
import { AboutPane } from './settings/AboutPane';
import { AppearancePane } from './settings/AppearancePane';
import { GeneralPane } from './settings/GeneralPane';
import { NotificationsPane } from './settings/NotificationsPane';
import { PrivacyPane } from './settings/PrivacyPane';
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
 *
 * Two things arrive from outside the window:
 *
 * - **A requested pane.** The popover's attention banners open Settings *at*
 *   the section that can fix what they reported. A window being created takes
 *   the request as it mounts; a window that already exists is told by event,
 *   because it is never re-mounted.
 * - **Nothing else.** There is no deep-link scheme and no route.
 *
 * The sidebar ends in Quit because a menu-bar application has no Dock icon and
 * no application menu: Settings and the tray menu are the only two places a
 * reader can reasonably look for the way out.
 */

const PANES: readonly (SidebarNavItem & { id: SettingsPane })[] = [
  { id: 'general', label: 'General', icon: SlidersHorizontal },
  { id: 'appearance', label: 'Appearance', icon: Palette },
  { id: 'sources', label: 'Sources', icon: FolderGit2 },
  { id: 'privacy', label: 'Privacy', icon: ShieldCheck },
  { id: 'notifications', label: 'Notifications', icon: Bell },
  { id: 'updates', label: 'Updates', icon: RefreshCw, separatorBefore: true },
  { id: 'about', label: 'About', icon: Info },
];

export function SettingsView() {
  const [pane, setPane] = useState<SettingsPane>('general');
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

  // A pane somebody asked for. Taken once on mount (the window may have been
  // created *by* that request), and listened for afterwards (a window that was
  // already open never mounts again). Unknown ids are ignored rather than
  // rendering nothing.
  useEffect(() => {
    let active = true;
    void takeSettingsPane()
      .then((requested) => {
        if (active && isSettingsPane(requested)) setPane(requested);
      })
      .catch(() => {});
    const pending = onSettingsPaneRequest((requested) => {
      if (active && isSettingsPane(requested)) setPane(requested);
    });
    return () => {
      active = false;
      void pending.then((unlisten) => unlisten());
    };
  }, []);

  return (
    <div className="flex h-full min-h-0">
      <SidebarNav
        items={PANES}
        value={pane}
        onChange={(next) => {
          if (isSettingsPane(next)) setPane(next);
        }}
        ariaLabel="Settings sections"
        footer={
          <button
            type="button"
            onClick={() => void quitApp()}
            className="type-body flex h-9 w-full items-center gap-3 rounded-control px-3 text-label transition-colors duration-[120ms] ease-out hover:bg-surface-hover"
          >
            <LogOut size={16} strokeWidth={2} className="shrink-0" aria-hidden="true" />
            <span className="truncate">Quit antiburn</span>
          </button>
        }
      />

      <div className="flex min-h-0 min-w-0 flex-1 flex-col">
        <ScrollPane viewportClassName="px-6 py-5">
          <div
            role="tabpanel"
            id={`${pane}-panel`}
            aria-labelledby={`${pane}-tab`}
            className="mx-auto w-full max-w-[520px]"
          >
            {pane === 'general' && <GeneralPane {...controller} info={info} />}
            {pane === 'appearance' && <AppearancePane {...controller} />}
            {pane === 'sources' && <SourcesPane />}
            {pane === 'privacy' && <PrivacyPane />}
            {pane === 'notifications' && <NotificationsPane {...controller} info={info} />}
            {pane === 'updates' && <UpdatesPane {...controller} info={info} />}
            {pane === 'about' && <AboutPane info={info} onOpenPane={setPane} />}
          </div>
        </ScrollPane>
      </div>
    </div>
  );
}
