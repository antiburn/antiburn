// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { render, screen } from '@testing-library/react';
import { Check } from 'lucide-react';
import { describe, expect, it } from 'vitest';

import { StatusText } from './StatusText';

describe('StatusText', () => {
  it('renders the message on its own, with no glyph', () => {
    render(<StatusText>Checking…</StatusText>);

    const status = screen.getByText('Checking…');
    expect(status.querySelector('svg')).toBeNull();
    expect(status.className).toContain('type-footnote');
    expect(status.className).toContain('text-label');
  });

  it('renders a decorative glyph at the footnote icon step', () => {
    render(
      <StatusText icon={Check} iconClassName="text-system-green" iconStrokeWidth={3}>
        Installed
      </StatusText>,
    );

    const glyph = screen.getByText('Installed').querySelector('svg')!;
    expect(glyph.getAttribute('aria-hidden')).toBe('true');
    expect(glyph.getAttribute('width')).toBe('12');
    expect(glyph.getAttribute('stroke-width')).toBe('3');
    expect(glyph.getAttribute('class')).toContain('shrink-0');
    expect(glyph.getAttribute('class')).toContain('text-system-green');
  });

  it('quiets the message in the secondary tone', () => {
    render(<StatusText tone="secondary">Not installed</StatusText>);

    expect(screen.getByText('Not installed').className).toContain('text-label-secondary');
  });

  it('accepts rich children so a value can be marked up inside the line', () => {
    render(
      <StatusText>
        Reading — <span className="tabular-nums">42%</span>
      </StatusText>,
    );

    expect(screen.getByText('42%').className).toContain('tabular-nums');
  });
});
