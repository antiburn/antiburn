// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { cleanup, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it } from 'vitest';

import { WslOriginBadge } from './WslOriginBadge';

afterEach(cleanup);

describe('WslOriginBadge', () => {
  it('identifies the distribution accessibly', () => {
    render(<WslOriginBadge distro="Ubuntu-24.04" />);
    expect(
      screen.getByLabelText('Found in Ubuntu-24.04 on Windows Subsystem for Linux'),
    ).toBeTruthy();
  });

  it('does not render for native origins', () => {
    const { container } = render(<WslOriginBadge />);
    expect(container.childElementCount).toBe(0);
  });

  it('renders an injected glyph in place of the default mark', () => {
    render(<WslOriginBadge distro="Debian" icon={<svg data-testid="custom-glyph" />} />);
    expect(screen.getByTestId('custom-glyph')).toBeTruthy();
  });
});
