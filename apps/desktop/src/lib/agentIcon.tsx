/** Agent icons preserve vendor colors or use neutral theme ink at the caller's request. */

import { Bot, PanelsTopLeft, SquareTerminal, type LucideIcon } from "lucide-react"
import type { ReactNode } from "react"
import { VENDOR_ARTWORK, type VendorArtwork } from "./vendorArtwork"

import {
  agentDisplayName,
  agentIconName,
  type AgentIconAppearance,
  type AgentSurface,
} from "./presentation/agents"

/** The glyph for a surface. Unknown surfaces get the neutral agent mark. */
function glyphFor(surface: AgentSurface | undefined): LucideIcon {
  if (surface === "cli") return SquareTerminal
  if (surface === "ide_desktop") return PanelsTopLeft
  return Bot
}

function Mark({
  size,
  appearance,
  artwork,
}: {
  size: number
  appearance: AgentIconAppearance
  artwork: VendorArtwork
}) {
  if (appearance === "default") {
    return (
      <>
        <img
          className="agent-icon-light"
          src={artwork.light}
          alt=""
          width={size}
          height={size}
          aria-hidden="true"
        />
        <img
          className="agent-icon-dark"
          src={artwork.dark}
          alt=""
          width={size}
          height={size}
          aria-hidden="true"
        />
      </>
    )
  }
  return (
    <span
      aria-hidden="true"
      className="inline-block bg-current"
      style={{
        width: size,
        height: size,
        maskImage: `url("${artwork.mask}")`,
        maskMode: artwork.maskMode,
        maskSize: "contain",
        maskPosition: "center",
        maskRepeat: "no-repeat",
      }}
    />
  )
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
  )
}

/**
 * Render one agent's icon.
 *
 * Matches the `SessionAgentIconRenderer` / `AgentIconRenderer` signatures the
 * presentation components declare, so it can be passed straight into either.
 */
export function renderAgentIcon(
  slug: string,
  size: number,
  surface?: AgentSurface,
  appearance: AgentIconAppearance = "default",
): ReactNode {
  const iconName = agentIconName(slug)
  const artwork = VENDOR_ARTWORK[iconName]
  const Glyph = glyphFor(surface)
  return (
    <span
      data-agent-icon={iconName}
      className={`inline-flex shrink-0 ${appearance === "neutral" ? "text-label-tertiary" : "text-agent-mark"}`}
      role="img"
      aria-label={agentDisplayName(slug)}
      title={agentDisplayName(slug)}
    >
      {artwork ? (
        <Mark size={size} appearance={appearance} artwork={artwork} />
      ) : iconName === "generic-agent" ? (
        <Glyph size={size} strokeWidth={1.75} aria-hidden="true" />
      ) : (
        <LetterTile name={agentDisplayName(slug)} size={size} />
      )}
    </span>
  )
}
