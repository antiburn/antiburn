// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { check } from '@tauri-apps/plugin-updater';
import { AlertTriangle, Check as CheckGlyph, Download, Loader2 } from 'lucide-react';
import { useCallback, useState } from 'react';

import { Card } from '../../components/ui/Card';
import { Pane } from '../../components/ui/Pane';
import { PushButton } from '../../components/ui/PushButton';
import { Row } from '../../components/ui/Row';
import { SectionGroup } from '../../components/ui/SectionGroup';
import { StatusText } from '../../components/ui/StatusText';
import { ToggleRow } from '../../components/ui/ToggleRow';
import type { AppInfo } from '../../lib/ipc';
import type { AppSettingsController } from './useAppSettings';

/**
 * Updates.
 *
 * The updater plugin is the **only** network-capable surface in the whole
 * application, and it is registered in release builds only. A development run
 * therefore says so plainly rather than running a check that would fail with an
 * opaque plugin error — an honest "unavailable" beats a misleading one.
 */

type CheckState =
  | { kind: 'idle' }
  | { kind: 'checking' }
  | { kind: 'current' }
  | { kind: 'available'; version: string }
  | { kind: 'failed'; message: string };

export interface UpdatesPaneProps extends AppSettingsController {
  /** Absent until the shell answers; `null` outside the shell entirely. */
  info: AppInfo | null;
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

export function UpdatesPane({ settings, update, info }: UpdatesPaneProps) {
  const [state, setState] = useState<CheckState>({ kind: 'idle' });
  const supported = info?.updatesSupported ?? false;

  const runCheck = useCallback(async () => {
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
    <Pane title="Updates">
      <SectionGroup title="Automatic updates">
        <Card>
          <ToggleRow
            label="Check for updates automatically"
            description="antiburn contacts the release feed only for this check. Nothing about your sessions is ever sent."
            checked={settings.autoUpdate}
            onChange={(next) => void update({ autoUpdate: next })}
            disabled={!supported}
            dimmed={!supported}
          />
          <Row
            label="Version"
            description={
              supported
                ? undefined
                : 'Updates are unavailable in this build — the updater is installed in packaged releases only.'
            }
            trailing={
              <div className="flex items-center gap-3">
                <span className="type-body tabular-nums text-label-secondary">
                  {info?.appVersion ?? '—'}
                </span>
                <PushButton onClick={() => void runCheck()} disabled={!supported}>
                  Check for updates
                </PushButton>
              </div>
            }
          >
            {supported && state.kind !== 'idle' && (
              <div className="mt-1.5">
                <CheckStatus state={state} />
              </div>
            )}
          </Row>
        </Card>
      </SectionGroup>
    </Pane>
  );
}
