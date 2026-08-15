// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { render } from '@testing-library/react';
import { siClaude, siCursor, siGithubcopilot, siWindsurf } from 'simple-icons';
import { describe, expect, it } from 'vitest';

import { ProviderGlyph, providerMarkPath } from './ProviderUsagePrimitives';

describe('ProviderGlyph — brand marks', () => {
  it('draws the provider’s own mark where a rights-cleared one exists', () => {
    const { container } = render(<ProviderGlyph displayName="Claude" provider="anthropic" />);
    expect(container.querySelector('[data-provider-icon="anthropic"]')).not.toBeNull();
    expect(container.querySelector('svg path')).toHaveAttribute('d');
  });

  it('falls back to the letter for a provider whose mark would be the wrong brand', () => {
    // `simple-icons` carries no OpenAI mark — its nearest entry belongs to a
    // different product — so OpenAI keeps its initial rather than wearing
    // someone else's logo. The same refusal the agent icons make for Amp,
    // where the package's `AMP` is Google's web framework.
    const { container } = render(<ProviderGlyph displayName="Codex" provider="openai" />);
    expect(container.querySelector('[data-provider-icon="letter"]')).toHaveTextContent('C');
    expect(container.querySelector('svg')).toBeNull();
    expect(providerMarkPath('openai')).toBeUndefined();
    // Same for xAI, where `siX` is the social network.
    expect(providerMarkPath('xai')).toBeUndefined();
  });

  it('falls back to the letter when no provider id is supplied at all', () => {
    const { container } = render(<ProviderGlyph displayName="Claude" />);
    expect(container.querySelector('[data-provider-icon="letter"]')).toHaveTextContent('C');
  });

  it('stays decorative, because every caller names the provider itself', () => {
    const { container } = render(<ProviderGlyph displayName="Claude" provider="anthropic" />);
    expect(container.firstElementChild).toHaveAttribute('aria-hidden', 'true');
  });

  it('exposes the mark path so the ring can draw the same identity', () => {
    expect(providerMarkPath('anthropic')).toBeTypeOf('string');
    expect(providerMarkPath('unknown')).toBeUndefined();
  });

  it('draws a vendor the same way the session cards already do', () => {
    // Anthropic wears the Claude mark, not the corporate one, because that is
    // what a session row shows. A reader scrolling from their sessions to
    // their usage should not have to work out that two marks mean one vendor.
    expect(providerMarkPath('anthropic')).toBe(siClaude.path);
    expect(providerMarkPath('cursor')).toBe(siCursor.path);
    expect(providerMarkPath('windsurf')).toBe(siWindsurf.path);
    expect(providerMarkPath('github')).toBe(siGithubcopilot.path);
  });
});
