// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { render } from '@testing-library/react';
import { describe, expect, it } from 'vitest';

import { UsageRing } from './UsageRing';

/** The arc's total length, which the dash offset is measured against. */
const CIRCUMFERENCE = 2 * Math.PI * 13;

function arcOffset(container: HTMLElement): number {
  const arc = container.querySelector('[data-testid="usage-ring-arc"]');
  return Number(arc?.getAttribute('stroke-dashoffset'));
}

describe('UsageRing', () => {
  it('fills the arc in proportion to what is consumed', () => {
    const { container } = render(<UsageRing percent={75} />);
    // Three quarters gone leaves a quarter of the circumference hidden.
    expect(arcOffset(container)).toBeCloseTo(CIRCUMFERENCE * 0.25, 5);
  });

  it('draws an empty ring rather than an arc at zero when nothing was stated', () => {
    // A ring at 0% is a claim that nothing has been used. A dashed track with
    // no arc is visibly a ring with no reading in it, which is the truth.
    const { container } = render(<UsageRing percent={null} />);
    expect(container.querySelector('[data-testid="usage-ring-arc"]')).toBeNull();
    expect(container.querySelector('circle')).toHaveAttribute('stroke-dasharray', '2 2');
  });

  it('renders a stated zero as a real, empty arc', () => {
    const { container } = render(<UsageRing percent={0} />);
    expect(arcOffset(container)).toBeCloseTo(CIRCUMFERENCE, 5);
    // Solid track, because this reading exists.
    expect(container.querySelector('circle')).not.toHaveAttribute('stroke-dasharray', '2 2');
  });

  it('marks a modelled figure as second-hand', () => {
    const { container } = render(<UsageRing percent={40} estimated />);
    expect(container.querySelector('[data-testid="usage-ring-estimated"]')).not.toBeNull();
    expect(
      render(<UsageRing percent={40} />).container.querySelector(
        '[data-testid="usage-ring-estimated"]',
      ),
    ).toBeNull();
  });

  it('clamps rather than overdrawing a figure past its own limit', () => {
    const { container } = render(<UsageRing percent={140} />);
    expect(arcOffset(container)).toBeCloseTo(0, 5);
  });

  it('keeps the provider’s identity inside the ring', () => {
    // A chip that says how full something is without saying whose is a worse
    // trade than the ring is worth.
    const { container } = render(<UsageRing percent={40} glyph="A" />);
    expect(container.querySelector('[data-testid="usage-ring-glyph"]')).toHaveTextContent('A');
    expect(
      render(<UsageRing percent={40} />).container.querySelector(
        '[data-testid="usage-ring-glyph"]',
      ),
    ).toBeNull();
  });

  it('prefers a brand mark over the letter when one exists', () => {
    // The letter is the fallback for providers with no rights-cleared mark,
    // not a second thing to draw alongside one.
    const { container } = render(<UsageRing percent={40} glyph="A" markPath="M0 0h24v24H0z" />);
    expect(container.querySelector('[data-testid="usage-ring-mark"]')).not.toBeNull();
    expect(container.querySelector('[data-testid="usage-ring-glyph"]')).toBeNull();
  });

  it('is invisible to a screen reader, because its caller names it', () => {
    // The ring is a shape with no text. Every call site puts the figure into
    // the accessible name of the control around it instead.
    const { container } = render(<UsageRing percent={40} />);
    expect(container.querySelector('svg')).toHaveAttribute('aria-hidden', 'true');
  });
});
