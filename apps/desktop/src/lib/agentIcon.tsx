// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

/**
 * Filling the presentation layer's agent-icon slot.
 *
 * The presentation components take a `renderAgentIcon` slot and never ship
 * artwork; the registry in `lib/presentation/agents` resolves a slug to an icon
 * *name*, not an asset. This module is the app's answer to that slot today: a
 * neutral, uniform glyph for every agent.
 *
 * That is deliberate rather than provisional. Vendor logos are the vendors'
 * marks, not ours to redistribute, so this app draws none. The resolved icon
 * name still rides along as a data attribute, so a future stream that
 * commissions original per-agent artwork has a seam to key off without
 * touching any call site.
 *
 * The glyph does carry one honest distinction: where the session came from. A
 * terminal for a CLI session, an editor panel for an IDE one, and a neutral
 * mark when the surface is unknown.
 */

import { Bot, PanelsTopLeft, SquareTerminal, type LucideIcon } from 'lucide-react';
import type { ReactNode } from 'react';

import { agentDisplayName, agentIconName, type AgentSurface } from './presentation/agents';

/** The glyph for a surface. Unknown surfaces get the neutral agent mark. */
function glyphFor(surface: AgentSurface | undefined): LucideIcon {
  if (surface === 'cli') return SquareTerminal;
  if (surface === 'ide_desktop') return PanelsTopLeft;
  return Bot;
}

/**
 * Render one agent's icon.
 *
 * Matches the `ActivityAgentIconRenderer` / `AgentIconRenderer` signatures the
 * presentation components declare, so it can be passed straight into either.
 */
export function renderAgentIcon(slug: string, size: number, surface?: AgentSurface): ReactNode {
  const Glyph = glyphFor(surface);
  return (
    <span
      // The seam: the registry's icon name for this slug, so artwork can be
      // introduced later without every call site learning about it.
      data-agent-icon={agentIconName(slug)}
      className="inline-flex items-center justify-center text-label-tertiary"
      // Decorative on its own — the row's title carries the session's name —
      // but the agent is not otherwise stated, so it gets a label.
      role="img"
      aria-label={agentDisplayName(slug)}
    >
      <Glyph size={size} strokeWidth={1.75} aria-hidden="true" />
    </span>
  );
}
