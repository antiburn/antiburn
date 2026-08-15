// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { render } from '@testing-library/react';
import { siClaude, siCursor, siGithubcopilot, siWindsurf } from 'simple-icons';
import { describe, expect, it } from 'vitest';

import { OPENAI_MARK } from '../../lib/brandMarks';
import { ProviderGlyph, providerMark } from './ProviderUsagePrimitives';

describe('ProviderGlyph — brand marks', () => {
  it('draws the provider’s own mark where a rights-cleared one exists', () => {
    const { container } = render(<ProviderGlyph displayName="Claude" provider="anthropic" />);
    expect(container.querySelector('[data-provider-icon="anthropic"]')).not.toBeNull();
    expect(container.querySelector('svg path')).toHaveAttribute('d');
  });

  it('falls back to the letter for a provider whose mark would be the wrong brand', () => {
    // `siX` is the social network, not the model provider, so xAI keeps its
    // initial rather than wearing someone else's logo. The same refusal the
    // agent icons make for Amp, where the package's `AMP` is Google's.
    const { container } = render(<ProviderGlyph displayName="xAI" provider="xai" />);
    expect(container.querySelector('[data-provider-icon="letter"]')).toHaveTextContent('X');
    expect(container.querySelector('svg')).toBeNull();
    expect(providerMark('xai')).toBeUndefined();
  });

  it('draws OpenAI with the same mark its Codex sessions carry', () => {
    // A reader scrolling from their sessions to their usage should not have to
    // work out that two marks mean one vendor. `simple-icons` has no OpenAI
    // entry, so both surfaces take it from `lib/brandMarks`.
    const { container } = render(<ProviderGlyph displayName="OpenAI" provider="openai" />);
    expect(container.querySelector('[data-provider-icon="openai"]')).not.toBeNull();
    expect(container.querySelector('svg path')).toHaveAttribute('d', OPENAI_MARK.path);
  });

  it('falls back to the letter when no provider id is supplied at all', () => {
    const { container } = render(<ProviderGlyph displayName="Claude" />);
    expect(container.querySelector('[data-provider-icon="letter"]')).toHaveTextContent('C');
  });

  it('stays decorative, because every caller names the provider itself', () => {
    const { container } = render(<ProviderGlyph displayName="Claude" provider="anthropic" />);
    expect(container.firstElementChild).toHaveAttribute('aria-hidden', 'true');
  });

  it('exposes the mark so the ring can draw the same identity', () => {
    expect(providerMark('anthropic')?.path).toBeTypeOf('string');
    expect(providerMark('unknown')).toBeUndefined();
  });

  it('draws a vendor the same way the session cards already do', () => {
    // Anthropic wears the Claude mark, not the corporate one, because that is
    // what a session row shows. A reader scrolling from their sessions to
    // their usage should not have to work out that two marks mean one vendor.
    expect(providerMark('anthropic')?.path).toBe(siClaude.path);
    expect(providerMark('cursor')?.path).toBe(siCursor.path);
    expect(providerMark('windsurf')?.path).toBe(siWindsurf.path);
    expect(providerMark('github')?.path).toBe(siGithubcopilot.path);
  });
});
