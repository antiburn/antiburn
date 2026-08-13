// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { Card } from '../../components/ui/Card';
import { Pane } from '../../components/ui/Pane';
import { Row } from '../../components/ui/Row';
import { SectionGroup } from '../../components/ui/SectionGroup';
import { ToggleRow } from '../../components/ui/ToggleRow';
import type { AppInfo } from '../../lib/ipc';
import type { AppSettingsController } from './useAppSettings';

/**
 * Notifications.
 *
 * There are exactly two, and the pane lists both rather than describing a
 * category: a reader deciding whether to allow notifications is entitled to
 * know precisely what they are agreeing to be interrupted by. The shell is the
 * only thing that posts one (`src-tauri/src/notifications.rs`) and enforces the
 * same two-level gate this pane presents.
 *
 * The update row follows the Updates pane's rule: a build with no updater never
 * finds a version, so it renders an explanation rather than a switch over a
 * notification that could not arrive.
 */

export interface NotificationsPaneProps extends AppSettingsController {
  /** Absent until the shell answers; `null` outside the shell entirely. */
  info: AppInfo | null;
}

export function NotificationsPane({ settings, update, info }: NotificationsPaneProps) {
  const on = settings.notificationsEnabled;
  const updatesSupported = info?.updatesSupported ?? false;

  return (
    <Pane title="Notifications">
      <SectionGroup title="Allow">
        <Card>
          <ToggleRow
            label="Notify me"
            description="antiburn interrupts you for the two things below and nothing else. There is no digest, no progress notification, and no marketing."
            checked={on}
            onChange={(next) => void update({ notificationsEnabled: next })}
          />
        </Card>
      </SectionGroup>

      <SectionGroup title="What antiburn will say">
        <Card>
          {updatesSupported ? (
            <ToggleRow
              label="A new version is available"
              description="Posted once per version, after a scheduled check finds one. A check you started yourself is never notified — you are already looking at the answer."
              checked={settings.notifyUpdateAvailable}
              onChange={(next) => void update({ notifyUpdateAvailable: next })}
              // Dimmed and inert because the master switch above is off — a
              // state, not a feature that is missing.
              dimmed={!on}
              disabled={!on}
            />
          ) : (
            <Row
              label="A new version is available"
              // Not a switch: this build has no updater, so the notification it
              // would control can never be posted.
              description="This build has no updater, so it never finds a version to tell you about. Packaged releases check on their own schedule."
              dimmed
            />
          )}
          <ToggleRow
            label="A scan could not finish"
            description="Posted once per run of antiburn, not once per pass: whatever stopped the first scan usually stops the next one too, and sixty notifications would say the same thing sixty times."
            checked={settings.notifyScanFailure}
            onChange={(next) => void update({ notifyScanFailure: next })}
            dimmed={!on}
            disabled={!on}
          />
        </Card>
      </SectionGroup>

      <SectionGroup title="Delivery">
        <Card>
          <Row
            label="Your system has the final say"
            // Stated rather than left to be discovered: a switch that is on
            // while the operating system silently drops every notification is
            // the most confusing state this pane can be in.
            description="Notifications are delivered by your operating system's own notification centre. If antiburn is turned off there, nothing arrives whatever is chosen here. Nothing about a notification leaves this machine."
          />
        </Card>
      </SectionGroup>
    </Pane>
  );
}
