// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';

import { Row } from './Row';

describe('Row', () => {
  it('renders the label, description and trailing content', () => {
    render(
      <Row
        label="Scan on launch"
        description="Look for new sessions as soon as the app starts."
        trailing={<button type="button">Manage</button>}
      />,
    );

    // Regular weight, not semibold: rows are peers inside a card, so the label
    // separates from its description by size and contrast alone.
    expect(screen.getByText('Scan on launch').className).toContain('type-body');
    const description = screen.getByText('Look for new sessions as soon as the app starts.');
    expect(description.classList.contains('col-span-2')).toBe(true);
    expect(screen.getByRole('button', { name: 'Manage' })).toBeTruthy();
  });

  it('omits the description paragraph when no description is given', () => {
    const { container } = render(<Row label="Bare row" />);

    expect(screen.getByText('Bare row')).toBeTruthy();
    expect(container.querySelectorAll('p')).toHaveLength(1);
  });

  it('dims the row when asked', () => {
    const { container } = render(<Row label="Dimmed" dimmed />);

    expect(container.firstElementChild?.classList.contains('opacity-50')).toBe(true);
  });

  it('renders extra full-width content below the description', () => {
    render(
      <Row label="Auto-dismiss" description="How long it stays.">
        <input type="range" aria-label="Auto-dismiss" />
      </Row>,
    );

    expect(screen.getByRole('slider', { name: 'Auto-dismiss' })).toBeTruthy();
  });
});
