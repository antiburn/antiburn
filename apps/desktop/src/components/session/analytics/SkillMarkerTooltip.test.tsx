// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';

import type { SkillDetail } from '../../../lib/types/session';
import type { TimelineMarker } from './markerCluster';
import { SkillMarkerTooltip } from './SkillMarkerTooltip';

afterEach(cleanup);

function marker(over: Partial<SkillDetail> = {}): TimelineMarker<SkillDetail> {
  const progress = over.progress ?? 0.4;
  return {
    progress,
    data: { name: 'deep-research', progress, tokensOut: 120, contextTokens: 800, ...over },
  };
}

describe('SkillMarkerTooltip', () => {
  it('frames a transcript description as coding-tool provenance', () => {
    render(<SkillMarkerTooltip members={[marker({ description: 'Fan-out research.' })]} />);
    expect(screen.getByText('Skill description from the coding tool:')).toBeTruthy();
    expect(screen.getByText('Fan-out research.')).toBeTruthy();
  });

  it("prefers the SKILL.md description over the transcript's one-liner", () => {
    render(
      <SkillMarkerTooltip
        members={[
          marker({
            description: 'Transcript one-liner.',
            local: {
              scope: 'global',
              available: true,
              description: 'Authoritative SKILL.md description.',
            },
          }),
        ]}
      />,
    );
    expect(screen.getByText('Authoritative SKILL.md description.')).toBeTruthy();
    expect(screen.queryByText('Transcript one-liner.')).toBeNull();
  });

  it('keeps provenance and file metadata behind the disclosure', () => {
    render(
      <SkillMarkerTooltip
        members={[
          marker({
            local: {
              version: '1.4.0',
              allowedTools: ['Read', 'Edit', 'Bash'],
              license: 'MIT',
              sizeBytes: 4300,
              scope: 'global',
              available: true,
            },
          }),
        ]}
      />,
    );
    expect(screen.queryByText(/3 tools/)).toBeNull();
    expect(screen.queryByText(/MIT/)).toBeNull();

    fireEvent.click(screen.getByText('More details'));
    expect(screen.getByText(/v1\.4\.0 · Global · 3 tools/)).toBeTruthy();
    expect(screen.getByText(/Read · Edit · Bash/)).toBeTruthy();
    expect(screen.getByText(/MIT · 4\.2 KiB/)).toBeTruthy();
  });

  it('marks a skill that is no longer on disk as removed', () => {
    render(
      <SkillMarkerTooltip
        members={[marker({ local: { version: '0.3.0', scope: 'project', available: false } })]}
      />,
    );
    fireEvent.click(screen.getByText('More details'));
    expect(screen.getByText(/Project · removed/)).toBeTruthy();
  });

  it('offers no disclosure for an unresolved skill with a short description', () => {
    render(<SkillMarkerTooltip members={[marker({ description: 'A built-in skill.' })]} />);
    expect(screen.getByText('A built-in skill.')).toBeTruthy();
    expect(screen.queryByText('More details')).toBeNull();
  });

  it('folds repeat invocations of one skill into a single counted card', () => {
    render(
      <SkillMarkerTooltip
        members={[
          marker({ progress: 0.2 }),
          marker({ progress: 0.5 }),
          marker({ progress: 0.7 }),
        ]}
      />,
    );
    expect(screen.getAllByText('deep-research')).toHaveLength(1);
    expect(screen.getByText(/×3/)).toBeTruthy();
    // The earliest invocation anchors the position.
    expect(screen.getByText(/20% through/)).toBeTruthy();
  });

  it('headlines a mixed cluster with the invocation count', () => {
    render(
      <SkillMarkerTooltip
        members={[marker(), marker({ name: 'checkpoint', progress: 0.6 })]}
      />,
    );
    expect(screen.getByText('2 skills used')).toBeTruthy();
  });

  it('shows a reveal control only when the host supplies the capability', () => {
    const local = { scope: 'global' as const, available: true, path: '/Users/dev/s/SKILL.md' };
    const { unmount } = render(<SkillMarkerTooltip members={[marker({ local })]} />);
    fireEvent.click(screen.getByText('More details'));
    // The path is still shown, home-collapsed; only the affordance is absent.
    expect(screen.getByText(/~\/s\/SKILL\.md/)).toBeTruthy();
    expect(screen.queryByRole('button', { name: /Reveal/ })).toBeNull();
    unmount();

    const onRevealPath = vi.fn();
    render(
      <SkillMarkerTooltip
        members={[marker({ local })]}
        onRevealPath={onRevealPath}
        revealLabel="Reveal in Finder"
      />,
    );
    fireEvent.click(screen.getByText('More details'));
    fireEvent.click(screen.getByRole('button', { name: 'Reveal in Finder' }));
    expect(onRevealPath).toHaveBeenCalledWith('/Users/dev/s/SKILL.md');
  });
});
