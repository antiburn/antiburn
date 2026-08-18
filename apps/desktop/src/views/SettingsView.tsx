// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import {
  Bell,
  Info,
  LogOut,
  Palette,
  ShieldCheck,
  SlidersHorizontal,
  FolderGit2,
  Gauge,
} from 'lucide-react';
import { useEffect, useLayoutEffect, useRef, useState } from 'react';

import { ScrollPane } from '../components/ui/ScrollPane';
import { SidebarNav, type SidebarNavItem } from '../components/ui/SidebarNav';
import {
  appInfo,
  closeCurrentWindow,
  onSettingsPaneRequest,
  quitApp,
  takeSettingsPane,
  type AppInfo,
} from '../lib/ipc';
import { isMacOS } from '../lib/platform';
import { isSettingsPane, type SettingsPane } from '../lib/settingsPanes';
import { AboutPane } from './settings/AboutPane';
import { AppearancePane } from './settings/AppearancePane';
import { GeneralPane } from './settings/GeneralPane';
import { NotificationsPane } from './settings/NotificationsPane';
import { PrivacyPane } from './settings/PrivacyPane';
import { SourcesPane } from './settings/SourcesPane';
import { UsagePane } from './settings/UsagePane';
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

// Everyday panes first, provenance last: Privacy and Notifications sit ahead
// of Sources and Appearance so the order survives a future where more panes
// exist, and About closes the list. Software update lives inside About, with
// the build it updates, rather than as a pane of its own.
const PANES: readonly (SidebarNavItem & { id: SettingsPane })[] = [
  { id: 'general', label: 'General', icon: SlidersHorizontal },
  { id: 'privacy', label: 'Privacy', icon: ShieldCheck },
  { id: 'notifications', label: 'Notifications', icon: Bell },
  { id: 'usage', label: 'Usage', icon: Gauge },
  { id: 'sources', label: 'Sources', icon: FolderGit2 },
  { id: 'appearance', label: 'Appearance', icon: Palette },
  { id: 'about', label: 'About', icon: Info },
];

export function SettingsView() {
  const [pane, setPane] = useState<SettingsPane>('general');
  const [info, setInfo] = useState<AppInfo | null>(null);
  const viewportRef = useRef<HTMLDivElement>(null);
  const controller = useAppSettings();

  useLayoutEffect(() => {
    if (viewportRef.current) viewportRef.current.scrollTop = 0;
  }, [pane]);

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

  // ⌘W closes the window. The shell runs as an accessory app with no
  // application menu, so the standard shortcut has no owner unless it is
  // handled here. Routed through a close *request* (which the shell turns into
  // a hide), so this takes the same path as the title-bar button.
  // Esc must NOT close: dismiss-on-Escape is modal behavior and a settings
  // window is not a modal. Do not "fix" this by adding Escape.
  useEffect(() => {
    function onKeyDown(event: KeyboardEvent) {
      if (
        (event.metaKey || event.ctrlKey) &&
        !event.altKey &&
        event.key.toLowerCase() === 'w'
      ) {
        event.preventDefault();
        void closeCurrentWindow();
      }
    }
    document.addEventListener('keydown', onKeyDown);
    return () => document.removeEventListener('keydown', onKeyDown);
  }, []);

  // On macOS the native title bar is hidden (overlay style in
  // `src-tauri/src/settings.rs`), so the content column pushes down past the
  // drag strip: pt-10 (40px) starts content exactly at the strip's bottom
  // edge. Windows/Linux keep the native bar and the original padding.
  const contentPadding = isMacOS() ? 'px-6 pb-5 pt-10' : 'px-6 py-5';

  return (
    <div className="relative flex h-full min-h-0">
      {isMacOS() && (
        // Custom title bar: the native one is hidden, so this transparent
        // strip is the window's drag handle. h-10 (40px) — taller than the
        // 28px overlay bar on purpose, a more forgiving grab target — and the
        // sidebar and content clearances both stop at its bottom edge, so
        // nothing interactive ever sits under it. No title text: the window
        // is already named by its sidebar. `data-tauri-drag-region` starts a
        // drag only when the mousedown lands on this element itself, so any
        // future child must be pointer-events-none; double-click no-ops
        // because the window is not resizable or maximizable.
        <div
          data-tauri-drag-region
          className="absolute inset-x-0 top-0 z-10 h-10"
          aria-hidden="true"
        />
      )}
      <SidebarNav
        items={PANES}
        value={pane}
        onChange={(next) => {
          if (isSettingsPane(next)) setPane(next);
        }}
        ariaLabel="Settings sections"
        // The overlay title bar hides the native bar: pt-7 plus the tablist's
        // own py-3 lands the first row at 40px — the bottom edge of the drag
        // strip — while the sidebar material still fills to the window's top
        // edge behind the traffic lights.
        className={isMacOS() ? 'pt-7' : ''}
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
        <ScrollPane viewportClassName={contentPadding} viewportRef={viewportRef}>
          {/* Keyed by pane so a section switch remounts the panel and plays
              the entrance once; the global reduced-motion clamp in
              styles/motion.css neutralizes it for readers who asked. */}
          <div
            key={pane}
            role="tabpanel"
            id={`${pane}-panel`}
            aria-labelledby={`${pane}-tab`}
            className="animate-step-in mx-auto w-full max-w-[600px]"
          >
            {pane === 'general' && <GeneralPane {...controller} info={info} />}
            {pane === 'appearance' && <AppearancePane {...controller} />}
            {pane === 'sources' && <SourcesPane />}
            {pane === 'privacy' && <PrivacyPane />}
            {pane === 'notifications' && <NotificationsPane {...controller} />}
            {pane === 'usage' && <UsagePane {...controller} />}
            {pane === 'about' && <AboutPane {...controller} info={info} onOpenPane={setPane} />}
          </div>
        </ScrollPane>
      </div>
    </div>
  );
}
