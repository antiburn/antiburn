// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

/**
 * Filling the presentation layer's agent-icon slot.
 *
 * The presentation components take a `renderAgentIcon` slot and never ship
 * artwork; the registry in `lib/presentation/agents` resolves a slug to an
 * icon *name*, not an asset. This module answers that slot with three tiers:
 *
 * 1. **A brand mark from `simple-icons`** (CC0-licensed path data) where the
 *    package carries the agent's actual mark — verified against each icon's
 *    recorded source so a name collision cannot smuggle in the wrong brand
 *    (the package's `AMP` is Google's web framework, not the Amp agent, so
 *    Amp deliberately has no entry here).
 * 2. **A letter tile** for known agents without a usable mark (Codex, Kiro,
 *    Amp, Antigravity): the display name's initial in a small rounded tile,
 *    the same treatment the provider glyphs use.
 * 3. **A surface glyph** for `generic-agent` — terminal for CLI, editor panel
 *    for IDE, neutral mark otherwise — since an unknown agent has no name to
 *    draw.
 *
 * Marks render in `currentColor` rather than each brand's hex, so they read
 * as one set in both themes. Their use is nominative — identifying which
 * vendor's agent produced a session — and is recorded in docs/deviations.md.
 */

import { Bot, PanelsTopLeft, SquareTerminal, type LucideIcon } from 'lucide-react';
import type { ReactNode } from 'react';
import {
  siClaude,
  siCline,
  siCursor,
  siGithubcopilot,
  siOpencode,
  siPi,
  siWindsurf,
  type SimpleIcon,
} from 'simple-icons';

import { agentDisplayName, agentIconName, type AgentSurface } from './presentation/agents';

/**
 * Registry icon name → brand mark. Keyed by the registry's icon names (not
 * slugs) so aliases like `claude-code` → `claude` resolve once, upstream.
 */
const BRAND_MARKS: Record<string, SimpleIcon> = {
  claude: siClaude,
  cursor: siCursor,
  copilot: siGithubcopilot,
  cline: siCline,
  opencode: siOpencode,
  windsurf: siWindsurf,
  pi: siPi,
};

/** The glyph for a surface. Unknown surfaces get the neutral agent mark. */
function glyphFor(surface: AgentSurface | undefined): LucideIcon {
  if (surface === 'cli') return SquareTerminal;
  if (surface === 'ide_desktop') return PanelsTopLeft;
  return Bot;
}

function BrandMark({ icon, size }: { icon: SimpleIcon; size: number }) {
  return (
    <svg viewBox="0 0 24 24" width={size} height={size} fill="currentColor" aria-hidden="true">
      <path d={icon.path} />
    </svg>
  );
}

/** The display name's initial, in the provider-glyph tile treatment. */
function LetterTile({ name, size }: { name: string; size: number }) {
  return (
    <span
      aria-hidden="true"
      className="inline-flex items-center justify-center rounded-[25%] border border-separator bg-surface-secondary font-medium leading-none"
      style={{ width: size, height: size, fontSize: Math.max(8, Math.round(size * 0.52)) }}
    >
      {name.charAt(0).toUpperCase()}
    </span>
  );
}

/**
 * Render one agent's icon.
 *
 * Matches the `ActivityAgentIconRenderer` / `AgentIconRenderer` signatures the
 * presentation components declare, so it can be passed straight into either.
 */
export function renderAgentIcon(slug: string, size: number, surface?: AgentSurface): ReactNode {
  const iconName = agentIconName(slug);
  const mark = BRAND_MARKS[iconName];
  const Glyph = glyphFor(surface);
  return (
    <span
      // The seam: the registry's icon name for this slug, so artwork can be
      // swapped later without every call site learning about it.
      data-agent-icon={iconName}
      className="inline-flex items-center justify-center text-label-secondary"
      // Decorative on its own — the row's title carries the session's name —
      // but the agent is not otherwise stated, so it gets a label.
      role="img"
      aria-label={agentDisplayName(slug)}
    >
      {mark ? (
        <BrandMark icon={mark} size={size} />
      ) : iconName === 'generic-agent' ? (
        <Glyph size={size} strokeWidth={1.75} aria-hidden="true" />
      ) : (
        <LetterTile name={agentDisplayName(slug)} size={size} />
      )}
    </span>
  );
}
