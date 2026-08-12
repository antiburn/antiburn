// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { render, screen } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { App } from './App';

// The shell is not attached under jsdom, so the IPC surface is stubbed at the
// module boundary: `isTauri()` reports presence and `invoke` answers commands.
const invoke = vi.hoisted(() => vi.fn());
vi.mock('@tauri-apps/api/core', () => ({
  invoke,
  isTauri: () => true,
}));

describe('App', () => {
  beforeEach(() => {
    invoke.mockReset();
    invoke.mockImplementation((command: string) => {
      if (command === 'engine_catalog_version') {
        return Promise.resolve('2026-01-01');
      }
      return Promise.resolve(null);
    });
    window.location.hash = '';
  });

  it('renders the popover with the engine-reported catalog version', async () => {
    render(<App />);

    expect(screen.getByRole('heading', { name: 'antiburn' })).toBeInTheDocument();
    expect(await screen.findByText('Pricing catalog 2026-01-01')).toBeInTheDocument();
    expect(invoke).toHaveBeenCalledWith('engine_catalog_version');
  });

  it('renders the settings view for the settings fragment', () => {
    window.location.hash = '#/settings';

    render(<App />);

    expect(screen.getByRole('heading', { name: 'Settings' })).toBeInTheDocument();
  });
});
