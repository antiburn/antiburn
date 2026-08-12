// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it } from 'vitest';

import { Tooltip } from './Tooltip';

afterEach(cleanup);

describe('Tooltip', () => {
  it('describes its trigger without rendering a resting surface', () => {
    render(
      <Tooltip label="Found in Ubuntu">
        <button type="button">Origin</button>
      </Tooltip>,
    );
    // Nothing is painted at rest; the trigger is still a plain, reachable control.
    expect(screen.queryByText('Found in Ubuntu')).toBeNull();
    expect(screen.getByRole('button', { name: 'Origin' })).toBeTruthy();
  });

  it('opens on focus and dismisses on blur', () => {
    render(
      <Tooltip label="Keyboard reachable" delayMs={0}>
        <button type="button">Origin</button>
      </Tooltip>,
    );
    const trigger = screen.getByRole('button', { name: 'Origin' });

    fireEvent.focus(trigger);
    expect(screen.getAllByText('Keyboard reachable').length).toBeGreaterThan(0);

    fireEvent.blur(trigger);
    expect(screen.queryByText('Keyboard reachable')).toBeNull();
  });

  it('toggles on click only in interactive mode', () => {
    const { rerender } = render(
      <Tooltip label="Tap target" interactive>
        <button type="button">Info</button>
      </Tooltip>,
    );
    const trigger = screen.getByRole('button', { name: 'Info' });

    fireEvent.click(trigger);
    expect(screen.getAllByText('Tap target').length).toBeGreaterThan(0);
    fireEvent.click(trigger);
    expect(screen.queryByText('Tap target')).toBeNull();

    rerender(
      <Tooltip label="Tap target">
        <button type="button">Info</button>
      </Tooltip>,
    );
    fireEvent.click(screen.getByRole('button', { name: 'Info' }));
    expect(screen.queryByText('Tap target')).toBeNull();
  });

  it('carries the shared platform tooltip surface class', () => {
    render(
      <Tooltip label="Styled" interactive>
        <button type="button">Info</button>
      </Tooltip>,
    );
    fireEvent.click(screen.getByRole('button', { name: 'Info' }));
    expect(document.querySelector('.ui-tooltip')).toBeTruthy();
  });
});
