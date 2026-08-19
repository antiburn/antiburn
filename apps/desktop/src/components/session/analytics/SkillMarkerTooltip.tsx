// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { ChevronDown, ChevronUp, FolderOpen, Zap } from "lucide-react"
import { Fragment, useCallback, useState } from "react"

import {
  formatBytes,
  formatCompact,
  formatDuration,
  formatRelativeTime,
} from "../../../lib/presentation/sessionAnalytics"
import type { LocalSkillDetails, SkillDetail, SkillScope } from "../../../lib/types/session"
import type { TimelineMarker } from "./markerCluster"

const SCOPE_LABEL: Record<SkillScope, string> = {
  global: "Global",
  project: "Project",
  plugin: "Plugin",
  unknown: "Unknown scope",
}

const FOOTER_BORDER = "1px solid color-mix(in srgb, var(--color-separator) 50%, transparent)"
/** Characters of tool names to show before rolling the rest into a "+N more". */
const TOOLS_CHAR_BUDGET = 56
/** Longest path rendered before the middle is elided. */
const PATH_MAX = 34
const DESCRIPTION_CLAMP_LINES = 2

interface SkillGroup {
  name: string
  count: number
  progress: number
  durationSecs: number
  tokensOut: number
  contextTokens: number
  description?: string | undefined
  local?: LocalSkillDetails | undefined
}

/**
 * Fold repeat invocations of the same skill into one card carrying a
 * recurrence count. A skill called six times in a burst is one fact about the
 * session, not six.
 */
function groupByName(members: TimelineMarker<SkillDetail>[]): SkillGroup[] {
  const groups = new Map<string, SkillGroup>()
  for (const { data } of members) {
    const existing = groups.get(data.name)
    if (existing) {
      existing.count += 1
      existing.progress = Math.min(existing.progress, data.progress)
      existing.durationSecs += Math.round((data.durationMs ?? 0) / 1000)
      existing.tokensOut += data.tokensOut
      existing.contextTokens = Math.max(existing.contextTokens, data.contextTokens)
      existing.description ??= data.local?.description ?? data.description
      existing.local ??= data.local
    } else {
      groups.set(data.name, {
        name: data.name,
        count: 1,
        progress: data.progress,
        durationSecs: Math.round((data.durationMs ?? 0) / 1000),
        tokensOut: data.tokensOut,
        contextTokens: data.contextTokens,
        description: data.local?.description ?? data.description,
        local: data.local,
      })
    }
  }
  return [...groups.values()]
}

function detailLine(group: SkillGroup): string {
  const pieces: string[] = []
  if (group.count > 1) pieces.push(`×${group.count}`)
  pieces.push(`${Math.round(group.progress * 100)}% through`)
  if (group.durationSecs > 0) pieces.push(formatDuration(group.durationSecs))
  if (group.tokensOut > 0) pieces.push(`${formatCompact(group.tokensOut)} out`)
  if (group.contextTokens > 0) pieces.push(`${formatCompact(group.contextTokens)} ctx`)
  return pieces.join(" · ")
}

function provenanceLine(local: LocalSkillDetails): string {
  const pieces: string[] = []
  if (local.version) pieces.push(`v${local.version}`)
  pieces.push(SCOPE_LABEL[local.scope])
  if (local.available === false) {
    pieces.push("removed")
  } else {
    const toolCount = local.allowedTools?.length ?? 0
    if (toolCount > 0) pieces.push(`${toolCount} ${toolCount === 1 ? "tool" : "tools"}`)
  }
  return pieces.join(" · ")
}

function fileMetaLine(local: LocalSkillDetails): string {
  const pieces: string[] = []
  if (local.license) pieces.push(local.license)
  if (local.sizeBytes && local.sizeBytes > 0) pieces.push(formatBytes(local.sizeBytes))
  if (local.modifiedAtMs) pieces.push(`edited ${formatRelativeTime(local.modifiedAtMs)}`)
  return pieces.join(" · ")
}

function toolsLine(tools: string[]): string {
  const shown: string[] = []
  let used = 0
  for (const tool of tools) {
    const add = (shown.length > 0 ? 3 : 0) + tool.length
    if (used + add > TOOLS_CHAR_BUDGET && shown.length > 0) break
    shown.push(tool)
    used += add
  }
  const rest = tools.length - shown.length
  const body = shown.join(" · ")
  return rest > 0 ? `${body} · +${rest} more` : body
}

/**
 * Shorten a path for a narrow card: collapse the user's home directory to `~`,
 * then elide the middle. The full path stays on the element's `title`.
 */
function displayPath(path: string): string {
  const tilde = path.replace(/^(\/Users\/[^/]+|\/home\/[^/]+|[A-Za-z]:\\Users\\[^\\]+)/, "~")
  if (tilde.length <= PATH_MAX) return tilde
  const head = tilde.slice(0, 9)
  const tail = tilde.slice(-(PATH_MAX - head.length - 1))
  return `${head}…${tail}`
}

function ClampedDescription({
  text,
  expanded,
  onOverflowChange,
}: {
  text: string
  expanded: boolean
  onOverflowChange: (overflows: boolean) => void
}) {
  // Measured via a ref callback rather than a layout effect: memoizing it on
  // the same deps the old effect had means React only detaches/reattaches it
  // (re-running the measurement) when `text` or `onOverflowChange` actually
  // changes — an `expanded` toggle alone leaves the identity, and so the
  // measurement, untouched, exactly like the effect's dependency array did.
  // Ref callbacks fire during commit, before paint, so the timing matches too.
  const measure = useCallback(
    (el: HTMLDivElement | null) => {
      if (el) onOverflowChange(el.scrollHeight > el.clientHeight + 1)
    },
    // `text` isn't read in the body above — it's here only to force the
    // ref-callback detach/reattach (see the doc comment) whenever it changes.
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [text, onOverflowChange],
  )
  return (
    <div
      ref={measure}
      className="mt-0.5 text-label-secondary"
      style={
        expanded
          ? undefined
          : {
              display: "-webkit-box",
              WebkitBoxOrient: "vertical",
              WebkitLineClamp: DESCRIPTION_CLAMP_LINES,
              overflow: "hidden",
            }
      }
    >
      {text}
    </div>
  )
}

function SkillDetails({
  local,
  onRevealPath,
  revealLabel,
}: {
  local: LocalSkillDetails
  onRevealPath?: (path: string) => void
  revealLabel: string
}) {
  const removed = local.available === false
  const tools = local.allowedTools ?? []
  const meta = fileMetaLine(local)
  const path = local.path
  return (
    <div
      className="mt-1 flex flex-col gap-0.5 pt-1 text-label-tertiary"
      style={{ borderTop: FOOTER_BORDER }}
    >
      <div>{provenanceLine(local)}</div>
      {!removed && tools.length > 0 && <div>Tools · {toolsLine(tools)}</div>}
      {!removed && meta && <div>{meta}</div>}
      {!removed && path && (
        <div className="flex items-center gap-1">
          <span className="truncate font-mono" title={path}>
            {displayPath(path)}
          </span>
          {onRevealPath && (
            <button
              type="button"
              onClick={() => onRevealPath(path)}
              title={revealLabel}
              aria-label={revealLabel}
              className="ml-auto shrink-0 cursor-pointer rounded p-0.5 text-label-tertiary transition-colors hover:bg-surface-secondary hover:text-system-gold-text"
            >
              <FolderOpen size={11} aria-hidden="true" />
            </button>
          )}
        </div>
      )}
    </div>
  )
}

function SkillCard({
  group,
  onRevealPath,
  revealLabel,
}: {
  group: SkillGroup
  onRevealPath?: (path: string) => void
  revealLabel: string
}) {
  const local = group.local
  const [expanded, setExpanded] = useState(false)
  const [descOverflows, setDescOverflows] = useState(false)
  // A card offers a disclosure only when it has something to disclose: either
  // a description the clamp cut off, or on-disk provenance to show.
  const canExpand = descOverflows || Boolean(local)

  return (
    <div className="px-1">
      <div className="flex items-center gap-1.5">
        <Zap size={12} aria-hidden="true" className="shrink-0 text-system-gold-text" />
        <span className="truncate font-medium text-label">{group.name}</span>
      </div>
      {group.description && (
        <div className="mt-0.5">
          <div className="text-label-tertiary">Skill description from the coding tool:</div>
          <ClampedDescription
            text={group.description}
            expanded={expanded}
            onOverflowChange={setDescOverflows}
          />
        </div>
      )}
      {expanded && local && (
        <SkillDetails
          local={local}
          {...(onRevealPath ? { onRevealPath } : {})}
          revealLabel={revealLabel}
        />
      )}
      {canExpand && (
        <button
          type="button"
          onClick={() => setExpanded((v) => !v)}
          className="mt-0.5 flex cursor-pointer items-center gap-0.5 text-system-gold-text hover:underline"
        >
          {expanded ? (
            <ChevronUp size={11} aria-hidden="true" />
          ) : (
            <ChevronDown size={11} aria-hidden="true" />
          )}
          <span>{expanded ? "Less details" : "More details"}</span>
        </button>
      )}
      <div className="mt-0.5 text-label-tertiary tabular-nums">{detailLine(group)}</div>
    </div>
  )
}

export interface SkillMarkerTooltipProps {
  members: TimelineMarker<SkillDetail>[]
  /**
   * Reveal a skill's `SKILL.md` wherever the host can. Omitted (the default)
   * hides the affordance entirely — the presentation layer knows nothing about
   * the filesystem, so the capability has to be handed to it.
   */
  onRevealPath?: (path: string) => void
  /** Accessible name for the reveal control, since the wording is host-specific. */
  revealLabel?: string
}

/** The hover card for one skill-marker cluster: one section per named skill. */
export function SkillMarkerTooltip({
  members,
  onRevealPath,
  revealLabel = "Reveal in file manager",
}: SkillMarkerTooltipProps) {
  const groups = groupByName(members)
  return (
    <div className="flex flex-col gap-1.5">
      {groups.length > 1 && (
        <div className="flex items-center gap-1.5 px-1">
          <Zap size={13} aria-hidden="true" className="shrink-0 text-system-gold-text" />
          <span className="type-callout text-system-gold-text">
            {members.length} skills used
          </span>
        </div>
      )}
      {/* Cap and scroll a long list; the header above stays pinned. */}
      <div
        className="-mr-1.5 flex flex-col gap-1.5 overflow-y-auto overscroll-contain pr-2.5"
        style={{ maxHeight: "min(260px, 55vh)" }}
      >
        {groups.map((group, i) => (
          <Fragment key={`skill-${i}`}>
            {i > 0 && <div style={{ borderTop: FOOTER_BORDER }} />}
            <SkillCard
              group={group}
              {...(onRevealPath ? { onRevealPath } : {})}
              revealLabel={revealLabel}
            />
          </Fragment>
        ))}
      </div>
    </div>
  )
}
