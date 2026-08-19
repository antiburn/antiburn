// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { check } from '@tauri-apps/plugin-updater';
import { AlertTriangle, Check as CheckGlyph, Download, Loader2 } from 'lucide-react';
import { useCallback, useSyncExternalStore } from 'react';

import { Card } from '../../components/ui/Card';
import { PushButton } from '../../components/ui/PushButton';
import { Row } from '../../components/ui/Row';
import { SectionGroup } from '../../components/ui/SectionGroup';
import { StatusText } from '../../components/ui/StatusText';
import { ToggleRow } from '../../components/ui/ToggleRow';
import { createExternalStore } from '../../lib/externalStore';
import { relativeTime } from '../../lib/presentation/relativeTime';
import { onUpdateStatus, type AppInfo, type UpdateStatusPayload } from '../../lib/ipc';
import type { AppSettingsController } from './useAppSettings';

/**
 * The Updates section of the About pane.
 *
 * A section rather than a sidebar pane of its own: software update belongs
 * with the build it updates, which is what About is. The masthead above
 * already names the version, so the row here is the action and its status,
 * not a restatement.
 *
 * The updater plugin is the **only** network-capable surface in the whole
 * application, and `info.updatesSupported` is the shell's answer about whether
 * it actually registered — not a compile-time guess. Everything here hangs off
 * that one flag:
 *
 * - **Supported.** The automatic-check switch is shown and is real: the shell
 *   runs the schedule, and the results arrive here as events, so the line under
 *   the switch reports what the last automatic check actually found.
 * - **Unsupported.** The switch is not rendered at all. A build that cannot
 *   check for updates has no automatic behaviour to configure, and a disabled
 *   switch over a preference nothing reads is the exact "control that does
 *   nothing" this section must not ship. The section says why instead.
 */

type CheckState =
  | { kind: 'idle' }
  | { kind: 'checking' }
  | { kind: 'current' }
  | { kind: 'available'; version: string }
  | { kind: 'failed'; message: string };

export interface UpdatesSectionProps extends AppSettingsController {
  /** Absent until the shell answers; `null` outside the shell entirely. */
  info: AppInfo | null;
}

/** Fold a shell-reported check outcome into the section's own state. */
function stateFromEvent(status: UpdateStatusPayload): CheckState {
  switch (status.kind) {
    case 'available':
      return { kind: 'available', version: status.version ?? '' };
    case 'current':
      return { kind: 'current' };
    case 'failed':
      return { kind: 'failed', message: status.message ?? 'The check could not be completed' };
    default:
      return { kind: 'idle' };
  }
}

function CheckStatus({ state }: { state: CheckState }) {
  switch (state.kind) {
    case 'checking':
      return (
        <StatusText icon={Loader2} iconClassName="animate-spin" tone="secondary">
          Checking…
        </StatusText>
      );
    case 'current':
      return (
        <StatusText icon={CheckGlyph} iconStrokeWidth={2.5} tone="secondary">
          Up to date
        </StatusText>
      );
    case 'available':
      return (
        <StatusText icon={Download} tone="primary">
          Version {state.version} is available
        </StatusText>
      );
    case 'failed':
      return (
        <StatusText icon={AlertTriangle} tone="secondary">
          {state.message}
        </StatusText>
      );
    default:
      return null;
  }
}

type UpdatesSnapshot = {
  state: CheckState;
  /** When the shell last checked on its own, as it reported it. */
  lastAutomatic: string | null;
};

// Module-level: the shell's automatic-check schedule and a manual check both
// land here regardless of how many settings windows are open to see them.
const updateStatusStore = createExternalStore<UpdatesSnapshot>({
  initial: { state: { kind: 'idle' }, lastAutomatic: null },
  // The shell owns the schedule, so this section learns about an automatic
  // check the same way it learns about anything else the shell did: an event.
  subscribe: (set) =>
    onUpdateStatus((status) => {
      const current = updateStatusStore.getSnapshot();
      set({
        state: stateFromEvent(status),
        lastAutomatic: status.automatic ? status.checkedAt : current.lastAutomatic,
      });
    }),
});

export function UpdatesSection({ settings, update, info }: UpdatesSectionProps) {
  const { state, lastAutomatic } = useSyncExternalStore(
    updateStatusStore.subscribe,
    updateStatusStore.getSnapshot,
  );
  const supported = info?.updatesSupported ?? false;

  const runCheck = useCallback(async () => {
    const setState = (next: CheckState) =>
      updateStatusStore.set({ ...updateStatusStore.getSnapshot(), state: next });
    setState({ kind: 'checking' });
    try {
      const found = await check();
      setState(found ? { kind: 'available', version: found.version } : { kind: 'current' });
    } catch (error) {
      setState({
        kind: 'failed',
        message: error instanceof Error ? error.message : 'The check could not be completed',
      });
    }
  }, []);

  return (
    <SectionGroup title="Updates">
      <Card>
        <Row
          label="Software update"
          description={
            supported
              ? undefined
              : 'Updates are unavailable in this build — the updater is installed in packaged releases only.'
          }
          trailing={
            <PushButton onClick={() => void runCheck()} disabled={!supported}>
              Check for updates
            </PushButton>
          }
        >
          {supported && state.kind !== 'idle' && (
            <div className="mt-1.5">
              <CheckStatus state={state} />
            </div>
          )}
        </Row>
        {supported ? (
          <ToggleRow
            label="Check for updates automatically"
            description={
              lastAutomatic
                ? `A moment after launch and every six hours. antiburn contacts the release feed for this check and nothing else; last checked ${relativeTime(lastAutomatic)}.`
                : 'A moment after launch and every six hours. antiburn contacts the release feed for this check and nothing else — nothing about your sessions is ever sent.'
            }
            checked={settings.autoUpdate}
            onChange={(next) => void update({ autoUpdate: next })}
          />
        ) : (
          <Row
            label="Automatic updates"
            // Not a disabled switch: there is no automatic behaviour in this
            // build to turn on or off, so there is nothing to render a
            // control for.
            description="This build has no updater, so it never contacts the release feed. Packaged releases check automatically."
          />
        )}
      </Card>
    </SectionGroup>
  );
}
