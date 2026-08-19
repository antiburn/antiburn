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
import { useState, useSyncExternalStore } from 'react';

import { ScrollPane } from '../components/ui/ScrollPane';
import { SidebarNav, type SidebarNavItem } from '../components/ui/SidebarNav';
import { closeCurrentWindow, quitApp } from '../lib/ipc';
import { useGlobalKeydown } from '../lib/useGlobalKeydown';
import { isMacOS } from '../lib/platform';
import { isSettingsPane, type SettingsPane } from '../lib/settingsPanes';
import { AboutPane } from './settings/AboutPane';
import { AppearancePane } from './settings/AppearancePane';
import { GeneralPane } from './settings/GeneralPane';
import { NotificationsPane } from './settings/NotificationsPane';
import { PrivacyPane } from './settings/PrivacyPane';
import { SettingsWindowSession } from './settings/SettingsWindowSession';
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
  const [session] = useState(() => new SettingsWindowSession());
  const { info, pane } = useSyncExternalStore(
    session.subscribe,
    session.getSnapshot,
    session.getSnapshot,
  );
  const controller = useAppSettings();

  // ⌘W closes the window. The shell runs as an accessory app with no
  // application menu, so the standard shortcut has no owner unless it is
  // handled here. Routed through a close *request* (which the shell turns into
  // a hide), so this takes the same path as the title-bar button.
  // Esc must NOT close: dismiss-on-Escape is modal behavior and a settings
  // window is not a modal. Do not "fix" this by adding Escape.
  useGlobalKeydown(true, (event) => {
    if ((event.metaKey || event.ctrlKey) && !event.altKey && event.key.toLowerCase() === 'w') {
      event.preventDefault();
      void closeCurrentWindow();
    }
  });

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
          if (isSettingsPane(next)) session.setPane(next);
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
        <ScrollPane viewportClassName={contentPadding}>
          {/* Keyed by pane so a section switch remounts the panel and plays
              the entrance once; the global reduced-motion clamp in
              styles/motion.css neutralizes it for readers who asked. The ref
              callback resets the shared scroll viewport when this node mounts
              — once per pane switch — in place of a layout effect keyed on
              `pane`. It reaches the viewport through the live DOM (`closest`)
              rather than a React ref: `ScrollPane` rebuilds its own viewport
              ref callback on every render, so a ref *object* can transiently
              read back `null` here (React attaches refs child-before-parent,
              and this node is the viewport's child) even though the viewport
              element itself — a plain DOM ancestor — is already in the tree by
              the time any ref for this commit runs. */}
          <div
            key={pane}
            ref={(node) => {
              const viewport = node?.closest<HTMLDivElement>('.ui-scroll-viewport');
              if (viewport) viewport.scrollTop = 0;
            }}
            role="tabpanel"
            id={`${pane}-panel`}
            aria-labelledby={`${pane}-tab`}
            className="animate-step-in mx-auto w-full max-w-[600px]"
          >
            {pane === 'general' && <GeneralPane {...controller} info={info} />}
            {pane === 'appearance' && <AppearancePane {...controller} />}
            {pane === 'sources' && (
              <SourcesPane discoveryPaused={controller.settings.discoveryPaused} />
            )}
            {pane === 'privacy' && <PrivacyPane />}
            {pane === 'notifications' && <NotificationsPane {...controller} />}
            {pane === 'usage' && <UsagePane {...controller} />}
            {pane === 'about' && (
              <AboutPane {...controller} info={info} onOpenPane={session.setPane} />
            )}
          </div>
        </ScrollPane>
      </div>
    </div>
  );
}
