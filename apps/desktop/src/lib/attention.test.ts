// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { describe, expect, it } from 'vitest';

import { attentionBanners } from './attention';
import { HEALTHY_STORAGE } from './ipc';

const repository = (repoName: string, status: string) => ({ repoName, status });

describe('attentionBanners', () => {
  it('says nothing when nothing is wrong', () => {
    expect(
      attentionBanners({
        repositories: [repository('widgets', 'accessible')],
        storage: HEALTHY_STORAGE,
      }),
    ).toEqual([]);
  });

  it('raises a source-access banner that goes to the pane which can fix it', () => {
    const [banner, ...rest] = attentionBanners({
      repositories: [repository('widgets', 'permission_denied')],
      storage: HEALTHY_STORAGE,
    });

    expect(rest).toEqual([]);
    expect(banner?.id).toBe('sourceAccess');
    expect(banner?.message).toContain('widgets');
    expect(banner?.actionLabel).toBe('Review');
    expect(banner?.action).toEqual({ kind: 'openSettings', pane: 'sources' });
    // Short, so a screen-reader user is not read the whole sentence twice.
    expect(banner?.dismissLabel).toBe('Dismiss the repository-access warning');
  });

  it('counts blocked repositories rather than listing them', () => {
    const [banner] = attentionBanners({
      repositories: [
        repository('widgets', 'permission_denied'),
        repository('gadgets', 'permission_denied'),
        repository('sprockets', 'accessible'),
      ],
      storage: HEALTHY_STORAGE,
    });

    expect(banner?.message).toContain('2 repositories');
  });

  it('ignores repositories that are simply not here, or turned off', () => {
    // `not_cloned` is a repository this machine does not have, and `disabled`
    // is one the reader chose to ignore. Neither is a problem to report.
    expect(
      attentionBanners({
        repositories: [repository('widgets', 'not_cloned'), repository('gadgets', 'disabled')],
        storage: HEALTHY_STORAGE,
      }),
    ).toEqual([]);
  });

  it('raises a storage banner whose action retries the failed work', () => {
    const [banner] = attentionBanners({
      repositories: [],
      storage: {
        failing: true,
        message: 'The session index could not be written: disk full',
      },
    });

    expect(banner?.id).toBe('storage');
    expect(banner?.message).toContain('disk full');
    // A storage failure must not read as data loss.
    expect(banner?.message).toContain('Nothing already indexed is lost');
    expect(banner?.actionLabel).toBe('Retry');
    expect(banner?.action).toEqual({ kind: 'rescan' });
  });

  it('still says something useful when the shell reported no detail', () => {
    const [banner] = attentionBanners({
      repositories: [],
      storage: { failing: true, message: null },
    });

    expect(banner?.message).toContain('could not write to its local database');
  });

  it('puts storage first: everything else is downstream of it', () => {
    const banners = attentionBanners({
      repositories: [repository('widgets', 'permission_denied')],
      storage: { failing: true, message: 'The session index could not be written: disk full' },
    });

    expect(banners.map((banner) => banner.id)).toEqual(['storage', 'sourceAccess']);
  });
});
