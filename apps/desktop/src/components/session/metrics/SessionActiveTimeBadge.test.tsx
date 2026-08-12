// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it } from 'vitest';

import { SessionActiveTimeBadge } from './SessionActiveTimeBadge';

afterEach(cleanup);

describe('SessionActiveTimeBadge', () => {
  it('leads with the active figure', () => {
    render(<SessionActiveTimeBadge activeSecs={5400} />);
    expect(screen.getAllByText('1h 30m').length).toBeGreaterThan(0);
  });

  it('shows the overall span beside the active one when both are known', () => {
    const { container } = render(
      <SessionActiveTimeBadge activeSecs={5400} durationSecs={28_800} />,
    );
    fireEvent.focus(container.querySelector('span[class*="rounded-full"]') as HTMLElement);
    expect(screen.getAllByText('Active time').length).toBeGreaterThan(0);
    expect(screen.getAllByText('Overall').length).toBeGreaterThan(0);
    expect(screen.getAllByText('8h').length).toBeGreaterThan(0);
    expect(screen.getAllByText('Excludes idle gaps').length).toBeGreaterThan(0);
  });

  it('drops the overall row rather than inventing a figure', () => {
    const { container } = render(<SessionActiveTimeBadge activeSecs={5400} />);
    fireEvent.focus(container.querySelector('span[class*="rounded-full"]') as HTMLElement);
    expect(screen.queryByText('Overall')).toBeNull();

    cleanup();
    const nulled = render(<SessionActiveTimeBadge activeSecs={5400} durationSecs={null} />);
    fireEvent.focus(
      nulled.container.querySelector('span[class*="rounded-full"]') as HTMLElement,
    );
    expect(screen.queryByText('Overall')).toBeNull();
  });

  it('appends caller classes to the pill', () => {
    const { container } = render(
      <SessionActiveTimeBadge activeSecs={60} className="relative top-px" />,
    );
    expect(container.querySelector('span[class*="relative top-px"]')).toBeTruthy();
  });
});
