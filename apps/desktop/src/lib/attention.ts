// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import type { StorageHealthPayload } from './ipc';
import type { SettingsPane } from './settingsPanes';

/**
 * Which attention banners the popover should be showing, derived from state the
 * app already has.
 *
 * Two rules decide what may appear here, and they are the reason this is a pure
 * function with its own tests rather than conditions scattered through the
 * view:
 *
 * 1. **A banner needs a real signal.** Every kind below is derived from
 *    something the shell reports — a repository the system refuses to let
 *    antiburn read, a database that rejected a write. There is no speculative
 *    kind, and there is no banner for a state the app cannot actually observe.
 * 2. **A banner needs somewhere to go.** Each carries exactly one action that
 *    either fixes the problem or retries the failed work. A banner that only
 *    announces bad news is a worse version of the status line.
 *
 * Update availability is deliberately *not* here. The Updates pane reports it,
 * a notification announces it once, and a third surface for the same fact would
 * be nagging.
 */

/** Which problem a banner describes. Doubles as its dismissal key. */
export type AttentionKind = 'storage' | 'sourceAccess';

/** What pressing a banner's action does. */
export type AttentionAction = { kind: 'openSettings'; pane: SettingsPane } | { kind: 'rescan' };

export interface AttentionBanner {
  id: AttentionKind;
  message: string;
  actionLabel: string;
  action: AttentionAction;
  /**
   * Accessible name for the dismissal. Short on purpose: the message is a whole
   * sentence, and a button whose name is a sentence is a button a screen-reader
   * user has to sit through twice.
   */
  dismissLabel: string;
}

/** The subset of a repository row this derivation reads. */
export interface AttentionRepository {
  repoName: string;
  /** `accessible`, `permission_denied`, `not_cloned`, or `disabled`. */
  status: string;
}

export interface AttentionInput {
  repositories: readonly AttentionRepository[];
  storage: StorageHealthPayload;
}

export function attentionBanners({ repositories, storage }: AttentionInput): AttentionBanner[] {
  const banners: AttentionBanner[] = [];

  // Storage first: when nothing can be written, every other problem is
  // downstream of this one.
  if (storage.failing) {
    banners.push({
      id: 'storage',
      message: `${storage.message ?? 'antiburn could not write to its local database'}. Nothing already indexed is lost, but new sessions are not being recorded.`,
      actionLabel: 'Retry',
      action: { kind: 'rescan' },
      dismissLabel: 'Dismiss the storage warning',
    });
  }

  // A repository the reader turned off reports `disabled`, never
  // `permission_denied`, so an ignored folder can never raise this.
  const blocked = repositories.filter(
    (repository) => repository.status === 'permission_denied',
  );
  if (blocked.length > 0) {
    const first = blocked[0]?.repoName ?? 'a repository';
    banners.push({
      id: 'sourceAccess',
      message:
        blocked.length === 1
          ? `The system is blocking antiburn from reading ${first}. Its sessions are missing from this list.`
          : `The system is blocking antiburn from reading ${blocked.length} repositories. Their sessions are missing from this list.`,
      actionLabel: 'Review',
      action: { kind: 'openSettings', pane: 'sources' },
      dismissLabel: 'Dismiss the repository-access warning',
    });
  }

  return banners;
}
