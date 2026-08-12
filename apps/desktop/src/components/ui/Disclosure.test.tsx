// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';

import { Disclosure, DisclosureGroup } from './Disclosure';

describe('Disclosure', () => {
  it('starts collapsed and keeps its body out of the tree', () => {
    render(<Disclosure label="How the local scan works">Body copy.</Disclosure>);

    const trigger = screen.getByRole('button', { name: 'How the local scan works' });
    expect(trigger.getAttribute('aria-expanded')).toBe('false');
    // Unmounted, not merely hidden — collapsed prose must not be reachable by
    // assistive tech or find-in-page.
    expect(screen.queryByText('Body copy.')).toBeNull();
  });

  it('reveals and hides the body on click, keeping aria-expanded in step', () => {
    render(<Disclosure label="Where session data lives">Body copy.</Disclosure>);
    const trigger = screen.getByRole('button', { name: 'Where session data lives' });

    fireEvent.click(trigger);
    expect(trigger.getAttribute('aria-expanded')).toBe('true');
    expect(screen.getByText('Body copy.')).toBeTruthy();

    fireEvent.click(trigger);
    expect(trigger.getAttribute('aria-expanded')).toBe('false');
    expect(screen.queryByText('Body copy.')).toBeNull();
  });

  it('points aria-controls at the body it reveals', () => {
    render(<Disclosure label="Retention">Body copy.</Disclosure>);
    const trigger = screen.getByRole('button', { name: 'Retention' });

    fireEvent.click(trigger);
    const controls = trigger.getAttribute('aria-controls');
    expect(controls).toBeTruthy();
    expect(document.getElementById(controls as string)?.textContent).toBe('Body copy.');
  });

  it('honors defaultOpen', () => {
    render(
      <Disclosure label="Open by default" defaultOpen>
        Body copy.
      </Disclosure>,
    );

    expect(
      screen.getByRole('button', { name: 'Open by default' }).getAttribute('aria-expanded'),
    ).toBe('true');
    expect(screen.getByText('Body copy.')).toBeTruthy();
  });

  it('gives each disclosure in a group its own independent state', () => {
    render(
      <DisclosureGroup>
        <Disclosure label="First">First body.</Disclosure>
        <Disclosure label="Second">Second body.</Disclosure>
      </DisclosureGroup>,
    );

    fireEvent.click(screen.getByRole('button', { name: 'First' }));
    expect(screen.getByText('First body.')).toBeTruthy();
    // Not an accordion: opening one must not close the others.
    expect(screen.queryByText('Second body.')).toBeNull();

    fireEvent.click(screen.getByRole('button', { name: 'Second' }));
    expect(screen.getByText('First body.')).toBeTruthy();
    expect(screen.getByText('Second body.')).toBeTruthy();
  });
});
