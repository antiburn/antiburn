// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { cleanup, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it } from 'vitest';

import { BRAND_COLORED, BRAND_MARKS, renderAgentIcon } from './agentIcon';
import {
  AGENT_SLUGS,
  agentDisplayName,
  agentIconName,
  GENERIC_AGENT_ICON,
  type AgentSurface,
} from './presentation/agents';

afterEach(cleanup);

/** Render one icon and hand back its wrapper. */
function renderIcon(slug: string, size = 18, surface?: AgentSurface): HTMLElement {
  render(<div data-testid="host">{renderAgentIcon(slug, size, surface)}</div>);
  const wrapper = screen.getByTestId('host').firstElementChild;
  if (!(wrapper instanceof HTMLElement)) throw new Error(`no icon rendered for ${slug}`);
  return wrapper;
}

describe('renderAgentIcon', () => {
  it.each([...AGENT_SLUGS])('labels %s and exposes its icon slot', (slug) => {
    const wrapper = renderIcon(slug);
    expect(wrapper).toHaveAttribute('role', 'img');
    expect(wrapper).toHaveAttribute('aria-label', agentDisplayName(slug));
    // The seam every call site relies on to swap artwork later.
    expect(wrapper).toHaveAttribute('data-agent-icon', agentIconName(slug));
  });

  it('draws a mark at the requested size for agents that have one', () => {
    const svg = renderIcon('claude-code', 20).querySelector('svg');
    expect(svg).not.toBeNull();
    expect(svg).toHaveAttribute('width', '20');
    expect(svg).toHaveAttribute('height', '20');
    expect(svg?.querySelector('path')?.getAttribute('d')).toBe(markOf('claude').path);
  });

  it('falls back to a letter tile for a known agent with no mark', () => {
    const wrapper = renderIcon('kiro');
    expect(wrapper.querySelector('svg')).toBeNull();
    expect(wrapper).toHaveTextContent('K');
  });

  it.each(['cli', 'ide_desktop', 'unknown'] as const)(
    'draws a surface glyph for an unknown slug on %s',
    (surface) => {
      const wrapper = renderIcon('not-a-real-agent', 18, surface);
      expect(wrapper).toHaveAttribute('data-agent-icon', GENERIC_AGENT_ICON);
      // Lucide glyphs are stroked SVGs, not filled paths.
      expect(wrapper.querySelector('svg')).not.toBeNull();
    },
  );
});

/** The mark registered for an icon slot, or a failure naming the missing one. */
function markOf(name: string) {
  const mark = BRAND_MARKS[name];
  if (!mark) throw new Error(`no brand mark registered for ${name}`);
  return mark;
}

/** A six-digit hex as the `rgb()` string jsdom reports for it. */
function rgbOf(hex: string): string {
  const n = Number.parseInt(hex, 16);
  return `rgb(${(n >> 16) & 255}, ${(n >> 8) & 255}, ${n & 255})`;
}

describe('brand colour', () => {
  it('renders brand-coloured marks in the mark’s own recorded hex', () => {
    const svg = renderIcon('claude-code').querySelector('svg');
    // jsdom normalizes an inline colour to its rgb() form.
    expect(svg?.style.color).toBe(rgbOf(markOf('claude').hex!));
  });

  it('leaves every other mark to inherit the theme ink', () => {
    for (const [name] of Object.entries(BRAND_MARKS)) {
      if (BRAND_COLORED.has(name)) continue;
      const slug = AGENT_SLUGS.find((s) => agentIconName(s) === name);
      expect(slug, `no slug resolves to ${name}`).toBeDefined();
      const svg = renderIcon(slug!).querySelector('svg');
      expect(svg?.style.color, `${name} should inherit ink`).toBe('');
      cleanup();
    }
  });

  it('gives every brand-coloured mark a hex to render', () => {
    // Guards the invariant the module comment states: the colour always comes
    // from the mark's recorded value, so a member without one is a bug.
    for (const name of BRAND_COLORED) {
      expect(markOf(name).hex, `${name} is brand-coloured but has no hex`).toMatch(
        /^[0-9A-Fa-f]{6}$/,
      );
    }
  });
});

describe('BRAND_MARKS', () => {
  it('has no key the registry never asks for', () => {
    const slots = new Set(AGENT_SLUGS.map(agentIconName));
    for (const name of Object.keys(BRAND_MARKS)) {
      expect(slots.has(name), `${name} is a dead key`).toBe(true);
    }
  });

  it('has no entry for Amp', () => {
    // `simple-icons`' AMP is Google's web framework, a name collision that
    // would put the wrong vendor's mark on an Amp row. Pinned here because the
    // decision is otherwise only a comment (docs/deviations.md D-18).
    expect(BRAND_MARKS[agentIconName('amp-code')]).toBeUndefined();
  });

  it('records provenance for every mark', () => {
    for (const [name, mark] of Object.entries(BRAND_MARKS)) {
      expect(mark.provenance.package, `${name} has no package`).toBeTruthy();
      expect(mark.provenance.license, `${name} has no licence`).toBe('CC0-1.0');
      expect(mark.provenance.source, `${name} has no source`).toMatch(/^https:\/\//);
      expect(mark.viewBox, `${name} has no viewBox`).toMatch(/^0 0 \d+ \d+$/);
    }
  });
});
