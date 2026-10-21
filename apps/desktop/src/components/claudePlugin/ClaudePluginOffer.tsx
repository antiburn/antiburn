import { Check, Copy, ExternalLink, X } from "lucide-react"
import { useRef, useState } from "react"

import {
  CLAUDE_PLUGIN_SESSION_COMMAND,
  CLAUDE_PLUGIN_SHELL_COMMANDS,
  CLAUDE_PLUGIN_SUMMARY,
  CLAUDE_PLUGIN_TAGLINE,
  CLAUDE_PLUGIN_TITLE,
  type ClaudePluginStatusPayload,
} from "../../lib/claudePluginOffer"
import type { ClaudePluginPlacement } from "../../lib/claudePluginStore"
import { writeClipboardText } from "../../lib/clipboard"
import { noteInteraction, openClaudePluginPage } from "../../lib/ipc"
import { PushButton } from "../ui/PushButton"

/**
 * The small dismissible card in the main-window sidebar footer. Install hands
 * the reader to the Agents page, where the full install detail lives.
 */
export function ClaudePluginSidebarCard({
  onInstall,
  onDismiss,
}: {
  onInstall: () => void
  onDismiss: () => void
}) {
  return (
    <div
      role="status"
      aria-label={CLAUDE_PLUGIN_TITLE}
      className="relative mb-2 rounded-control border border-separator bg-surface-card p-3"
    >
      <button
        type="button"
        onClick={onDismiss}
        aria-label="Dismiss"
        className="absolute top-1.5 right-1.5 flex size-5 items-center justify-center rounded-control text-label-tertiary hover:bg-surface-hover hover:text-label"
      >
        <X size={12} strokeWidth={2} aria-hidden="true" />
      </button>
      <h3 className="type-headline mr-4 text-label">{CLAUDE_PLUGIN_TITLE}</h3>
      <p className="mt-0.5 mb-2 type-footnote text-pretty text-label-secondary">
        {CLAUDE_PLUGIN_TAGLINE}
      </p>
      <PushButton onClick={onInstall}>Install</PushButton>
    </div>
  )
}

/** "Installed 0.2.1", with a note when Claude Code has the plugin turned off. */
export function claudePluginInstalledLine(status: ClaudePluginStatusPayload): string {
  const version = status.version ? ` ${status.version}` : ""
  const enabled = status.enabled ? "" : " Turned off in Claude Code's settings."
  return `Installed${version}.${enabled}`
}

/**
 * How to install, in two routes, plus what the plugin shows. The Agents page
 * and the Settings → Usage row both show this.
 */
export function ClaudePluginInstallDetail({ placement }: { placement: ClaudePluginPlacement }) {
  return (
    <div className="flex flex-col gap-3">
      <CommandBlock
        lead="In any Claude Code session, type:"
        lines={[CLAUDE_PLUGIN_SESSION_COMMAND]}
        copyLabel="Copy session command"
        onCopied={() =>
          noteInteraction({
            kind: "claudePluginOffer",
            action: "copy_session_command",
            placement,
          })
        }
      />
      <CommandBlock
        lead="Or from your shell:"
        lines={CLAUDE_PLUGIN_SHELL_COMMANDS}
        copyLabel="Copy shell commands"
        onCopied={() =>
          noteInteraction({
            kind: "claudePluginOffer",
            action: "copy_shell_command",
            placement,
          })
        }
      />
      <div className="flex items-start justify-between gap-3">
        <p className="type-footnote text-pretty text-label-secondary">
          <span className="text-label">What it shows: </span>
          {CLAUDE_PLUGIN_SUMMARY}
        </p>
        <PluginPageButton placement={placement} />
      </div>
    </div>
  )
}

export function PluginPageButton({ placement }: { placement: ClaudePluginPlacement }) {
  return (
    <PushButton
      trailingIcon={ExternalLink}
      onClick={() => {
        noteInteraction({ kind: "claudePluginOffer", action: "open_plugin_page", placement })
        void openClaudePluginPage().catch(() => undefined)
      }}
    >
      Plugin page
    </PushButton>
  )
}

const COPIED_RESET_MS = 2_000

function CommandBlock({
  lead,
  lines,
  copyLabel,
  onCopied,
}: {
  lead: string
  lines: readonly string[]
  copyLabel: string
  onCopied: () => void
}) {
  const [copied, setCopied] = useState(false)
  const resetTimer = useRef<ReturnType<typeof setTimeout> | null>(null)
  async function copy(): Promise<void> {
    try {
      await writeClipboardText(lines.join("\n"))
    } catch {
      return
    }
    onCopied()
    setCopied(true)
    if (resetTimer.current) clearTimeout(resetTimer.current)
    resetTimer.current = setTimeout(() => setCopied(false), COPIED_RESET_MS)
  }
  return (
    <div className="flex flex-col gap-1.5">
      <p className="type-footnote text-label-secondary">{lead}</p>
      <div className="flex items-start gap-2">
        <pre className="min-w-0 flex-1 overflow-x-auto rounded-control bg-surface-tertiary/40 px-2 py-1.5 font-mono type-caption text-label">
          {lines.join("\n")}
        </pre>
        <PushButton
          ariaLabel={copied ? "Copied" : copyLabel}
          onClick={() => void copy()}
          trailingIcon={copied ? Check : Copy}
        >
          {copied ? "Copied" : "Copy"}
        </PushButton>
      </div>
    </div>
  )
}
