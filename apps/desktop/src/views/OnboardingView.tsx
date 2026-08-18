// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import {
  useState,
  useSyncExternalStore,
  type KeyboardEvent as ReactKeyboardEvent,
} from 'react';

import { PushButton } from '../components/ui/PushButton';
import { closeCurrentWindow } from '../lib/ipc';
import { OnboardingFlow } from './onboarding/OnboardingFlow';
import { OnboardingSession } from './onboarding/OnboardingSession';

/**
 * The first-run window.
 *
 * The component only renders an immutable snapshot and routes reader actions.
 * IPC loading, scan events, permission prompts and command lifetimes belong to
 * `OnboardingSession`, the external-system boundary subscribed below.
 */
export function OnboardingView() {
  const [session] = useState(() => new OnboardingSession());
  const state = useSyncExternalStore(
    session.subscribe,
    session.getSnapshot,
    session.getSnapshot,
  );

  if (state.loadState === 'loading') {
    return (
      <div
        className="flex h-full items-center justify-center type-footnote text-label-secondary"
        aria-busy
      >
        Preparing antiburn…
      </div>
    );
  }

  if (state.loadState === 'error') {
    return (
      <div className="flex h-full flex-col items-center justify-center gap-3 px-8 text-center">
        <h1 className="type-title-3 text-label">antiburn could not start setup</h1>
        <p className="type-footnote text-label-secondary">{state.loadError}</p>
        <PushButton variant="primary" onClick={session.retry}>
          Try again
        </PushButton>
      </div>
    );
  }

  const blockedRoots = blockedDefaultRoots(
    state.defaultRoots,
    state.permissions.supported,
    state.permissions.deferred.map((entry) => entry.dir),
  );

  return (
    <div className="h-full" onKeyDownCapture={handleWindowKeyDown}>
      <OnboardingFlow
        defaultRoots={state.defaultRoots}
        blockedRoots={blockedRoots}
        permissions={state.permissions}
        permissionFlow={state.permissionFlow}
        onRecheckPermissions={session.recheckPermissions}
        recheckingPermissions={state.recheckingPermissions}
        scanRoots={state.scanRoots}
        onAddScanRoot={session.addScanRoot}
        onRemoveScanRoot={session.removeScanRoot}
        repositories={state.repositories}
        onToggleRepository={session.toggleRepository}
        onDiscover={session.rescan}
        onCancelScan={session.cancelScan}
        scanStatus={state.scanStatus}
        windowDays={state.activityWindowDays}
        onWindowDaysChange={session.setActivityWindowDays}
        launchAtLogin={state.launchAtLogin}
        onLaunchAtLoginChange={session.setLaunchAtLogin}
        finishing={state.finishing}
        finishError={state.finishError}
        onFinish={session.finish}
      />
    </div>
  );
}

function handleWindowKeyDown(event: ReactKeyboardEvent<HTMLDivElement>): void {
  if ((event.metaKey || event.ctrlKey) && !event.altKey && event.key.toLowerCase() === 'w') {
    event.preventDefault();
    void closeCurrentWindow();
  }
}

function blockedDefaultRoots(
  defaultRoots: string[],
  permissionsSupported: boolean,
  blockedDirectories: string[],
): string[] {
  if (!permissionsSupported || blockedDirectories.length === 0) return [];
  return defaultRoots.filter((root) =>
    blockedDirectories.some((dir) => root.split(/[\\/]/).includes(dir)),
  );
}
