import * as DropdownMenu from "@radix-ui/react-dropdown-menu"
import { Ellipsis, FolderOpen, Plus } from "lucide-react"
import { useRef, useState } from "react"
import { createPortal } from "react-dom"

import { Card } from "../../components/ui/Card"
import { PushButton } from "../../components/ui/PushButton"
import { StatusText } from "../../components/ui/StatusText"
import { renderAgentIcon } from "../../lib/agentIcon"
import {
  addedProfileCount,
  type ClaudeProfile,
  type ClaudeProfileSuggestion,
  type ClaudeProfilesPayload,
} from "../../lib/claudeProfiles"
import { SettingsSectionGroup } from "./SettingsSearchRows"
import { makeDialogBackgroundInert, trapDialogFocus } from "./settingsDialog"
import type { SourcesSession } from "./SourcesSession"

const INPUT_CLASS =
  "mt-1 h-[var(--control-height-regular)] w-full rounded-control border border-separator bg-input-fill px-2 type-body text-label"

/** The shell rejects with a reader-facing sentence; anything else is generic. */
function errorMessage(error: unknown, fallback: string): string {
  return typeof error === "string" && error.trim() ? error : fallback
}

/** A long path keeps its end visible, which is the part that differs. */
function PathText({ path }: { path: string }) {
  return (
    <span
      dir="rtl"
      title={path}
      className="block min-w-0 truncate text-left font-mono type-caption text-label-tertiary"
    >
      <bdi>{path}</bdi>
    </span>
  )
}

type EditorTarget =
  | { kind: "add"; suggestion: ClaudeProfileSuggestion | null }
  | { kind: "rename"; profile: ClaudeProfile }

function ClaudeProfileEditor({
  target,
  session,
  maxLabelChars,
  onClose,
}: {
  target: EditorTarget
  session: SourcesSession
  maxLabelChars: number
  onClose: () => void
}) {
  const initialLabel =
    target.kind === "rename" ? target.profile.label : (target.suggestion?.label ?? "")
  const [label, setLabel] = useState(initialLabel)
  const [path, setPath] = useState(
    target.kind === "rename" ? target.profile.path : (target.suggestion?.path ?? ""),
  )
  const [busy, setBusy] = useState(false)
  const [message, setMessage] = useState<string | null>(null)
  const name = label.trim()
  const adding = target.kind === "add"
  const dirty = adding ? name !== "" || path !== "" : name !== target.profile.label

  async function choose() {
    const picked = await session.pickClaudeProfileFolder().catch(() => null)
    if (picked) {
      setPath(picked)
      setMessage(null)
    }
  }

  async function save() {
    if (!name) {
      setMessage("Enter a name for this profile.")
      return
    }
    if (adding && !path) {
      setMessage("Choose the Claude Code folder for this profile.")
      return
    }
    setBusy(true)
    setMessage(null)
    try {
      if (target.kind === "rename") await session.renameClaudeProfile(target.profile.id, name)
      else await session.addClaudeProfile(name, path)
      onClose()
    } catch (error) {
      setMessage(errorMessage(error, "Could not save this profile. Try again."))
    } finally {
      setBusy(false)
    }
  }

  const titleId = `claude-profile-${target.kind}-title`
  const actionLabel = busy ? "Saving…" : adding ? "Add profile" : "Save changes"
  return createPortal(
    <div
      ref={makeDialogBackgroundInert}
      className="fixed inset-0 z-50 flex items-center justify-center bg-surface-window/80 p-6 backdrop-blur-sm"
      onMouseDown={(event) => {
        if (event.target === event.currentTarget && !busy) onClose()
      }}
    >
      <section
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        aria-busy={busy}
        onKeyDown={(event) => trapDialogFocus(event, () => !busy && onClose())}
        className="w-full max-w-md rounded-control border border-separator bg-surface-overlay p-5 text-label shadow-raised"
      >
        <h3 id={titleId} className="type-title-3 text-label">
          {adding ? "Add Claude profile" : "Rename Claude profile"}
        </h3>
        {adding ? (
          <p className="mt-2 type-footnote text-label-secondary">
            Choose the folder that Claude Code uses as its CLAUDE_CONFIG_DIR for this
            subscription.
          </p>
        ) : null}
        <label className="mt-4 block type-footnote text-label-secondary">
          Name
          <input
            autoFocus
            disabled={busy}
            value={label}
            maxLength={maxLabelChars > 0 ? maxLabelChars : undefined}
            onChange={(event) => {
              setLabel(event.target.value)
              setMessage(null)
            }}
            onKeyDown={(event) => {
              if (event.key === "Enter") void save()
            }}
            className={INPUT_CLASS}
            placeholder="Claude Work"
            aria-invalid={!!message && !name}
          />
        </label>
        <div className="mt-3 type-footnote text-label-secondary">
          Folder
          <div className="mt-1 flex items-center gap-2">
            <div className="flex h-[var(--control-height-regular)] min-w-0 flex-1 items-center rounded-control border border-separator bg-input-fill px-2">
              {path ? (
                <PathText path={path} />
              ) : (
                <span className="type-caption text-label-tertiary">No folder chosen</span>
              )}
            </div>
            {adding ? (
              <PushButton
                className="shrink-0 gap-1.5"
                disabled={busy}
                onClick={() => void choose()}
              >
                <FolderOpen size={12} aria-hidden="true" />
                Choose…
              </PushButton>
            ) : null}
          </div>
        </div>
        {message ? (
          <p role="alert" className="mt-3 type-footnote text-system-red-text">
            {message}
          </p>
        ) : null}
        <div className="mt-5 flex justify-end gap-2">
          <PushButton onClick={onClose} disabled={busy}>
            Cancel
          </PushButton>
          <PushButton onClick={() => void save()} disabled={busy || !dirty} variant="primary">
            <span className="inline-grid">
              <span aria-hidden="true" className="invisible col-start-1 row-start-1">
                {adding ? "Add profile" : "Save changes"}
              </span>
              <span className="col-start-1 row-start-1">{actionLabel}</span>
            </span>
          </PushButton>
        </div>
      </section>
    </div>,
    document.body,
  )
}

function RemoveClaudeProfileDialog({
  profile,
  session,
  onClose,
}: {
  profile: ClaudeProfile
  session: SourcesSession
  onClose: () => void
}) {
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  return createPortal(
    <div
      ref={makeDialogBackgroundInert}
      className="fixed inset-0 z-50 flex items-center justify-center bg-surface-window/80 p-6 backdrop-blur-sm"
    >
      <section
        role="alertdialog"
        aria-modal="true"
        aria-labelledby="remove-claude-profile-title"
        onKeyDown={(event) => trapDialogFocus(event, () => !busy && onClose())}
        className="w-full max-w-md rounded-control border border-separator bg-surface-overlay p-5 text-label shadow-raised"
      >
        <h3 id="remove-claude-profile-title" className="type-title-3">
          Remove {profile.label}?
        </h3>
        <p className="mt-2 type-body text-label-secondary">
          antiburn stops reading new sessions and usage limits from this folder. Sessions it
          already indexed stay, and nothing in the folder changes.
        </p>
        {error ? (
          <p role="alert" className="mt-3 type-footnote text-system-red-text">
            {error}
          </p>
        ) : null}
        <div className="mt-5 flex justify-end gap-2">
          <PushButton onClick={onClose} disabled={busy} autoFocus>
            Cancel
          </PushButton>
          <button
            type="button"
            disabled={busy}
            onClick={() => {
              setBusy(true)
              setError(null)
              session
                .removeClaudeProfile(profile.id)
                .then(onClose)
                .catch((reason: unknown) => {
                  setBusy(false)
                  setError(errorMessage(reason, "Could not remove this profile. Try again."))
                })
            }}
            className="ui-push-button border-transparent bg-system-red text-white disabled:opacity-50"
          >
            {busy ? "Removing…" : "Remove"}
          </button>
        </div>
      </section>
    </div>,
    document.body,
  )
}

function ClaudeProfileRow({
  profile,
  onRename,
  onRemove,
}: {
  profile: ClaudeProfile
  onRename: (trigger: HTMLElement) => void
  onRemove: (trigger: HTMLElement) => void
}) {
  const trigger = useRef<HTMLButtonElement>(null)
  return (
    <div className="flex items-center gap-2.5 px-4 py-2">
      <span className="flex w-5 shrink-0 justify-center">
        {renderAgentIcon("claude-code", 15)}
      </span>
      <div className="min-w-0 flex-1">
        <p className="flex items-center gap-2 type-body text-label">
          <span className="truncate">{profile.label}</span>
          {profile.builtIn ? (
            <span className="inline-flex h-[var(--space-lg)] shrink-0 items-center rounded-full bg-surface-tertiary/40 px-1.5 type-caption text-label-tertiary">
              Default
            </span>
          ) : null}
        </p>
        <PathText path={profile.path} />
      </div>
      <DropdownMenu.Root modal={false}>
        <DropdownMenu.Trigger asChild>
          <button
            ref={trigger}
            type="button"
            aria-label={`More actions for ${profile.label}`}
            className="ui-push-button flex shrink-0 items-center justify-center"
          >
            <Ellipsis size={14} aria-hidden="true" />
          </button>
        </DropdownMenu.Trigger>
        <DropdownMenu.Portal>
          <DropdownMenu.Content className="ui-menu min-w-32" align="end" sideOffset={4}>
            <DropdownMenu.Item
              className="ui-menu-item"
              onSelect={() => {
                if (trigger.current) onRename(trigger.current)
              }}
            >
              Rename
            </DropdownMenu.Item>
            {profile.builtIn ? null : (
              <DropdownMenu.Item
                className="ui-menu-item text-system-red-text"
                onSelect={() => {
                  if (trigger.current) onRemove(trigger.current)
                }}
              >
                Remove
              </DropdownMenu.Item>
            )}
          </DropdownMenu.Content>
        </DropdownMenu.Portal>
      </DropdownMenu.Root>
    </div>
  )
}

/**
 * Claude Code profiles: one row per configuration directory, each with the
 * reader's own name. The built-in row is the CLI's default directory. The
 * reader adds a row for each other `CLAUDE_CONFIG_DIR` they use, and antiburn
 * reads that folder's sessions and its usage limits under that name.
 */
export function ClaudeProfilesSection({
  claudeProfiles,
  session,
}: {
  claudeProfiles: ClaudeProfilesPayload
  session: SourcesSession
}) {
  const [editor, setEditor] = useState<{
    target: EditorTarget
    restoreFocus: HTMLElement
  } | null>(null)
  const [removing, setRemoving] = useState<{
    profile: ClaudeProfile
    restoreFocus: HTMLElement
  } | null>(null)
  const added = addedProfileCount(claudeProfiles)
  const atLimit = claudeProfiles.maxProfiles > 0 && added >= claudeProfiles.maxProfiles

  function restoreFocus(target: HTMLElement) {
    queueMicrotask(() => {
      const fallback = document.getElementById("add-claude-profile")
      ;(target.isConnected ? target : fallback)?.focus()
    })
  }

  function openEditor(target: EditorTarget) {
    const trigger = document.activeElement
    if (trigger instanceof HTMLElement) setEditor({ target, restoreFocus: trigger })
  }

  return (
    <>
      <SettingsSectionGroup
        searchId="sourceClaudeProfiles"
        trailing={
          <StatusText tone="secondary">
            {added === 0 ? "Default only" : `${added + 1} profiles`}
          </StatusText>
        }
      >
        <Card>
          <p className="px-4 pt-3 pb-1 type-footnote text-label-secondary">
            Add a profile for each Claude Code folder you sign in to, such as one per
            subscription. antiburn reads each folder’s sessions and shows its usage limits under
            the name you choose.
          </p>
          {claudeProfiles.profiles.map((profile) => (
            <ClaudeProfileRow
              key={profile.id}
              profile={profile}
              onRename={(trigger) =>
                setEditor({ target: { kind: "rename", profile }, restoreFocus: trigger })
              }
              onRemove={(trigger) => setRemoving({ profile, restoreFocus: trigger })}
            />
          ))}
          {claudeProfiles.suggestions.length > 0 && !atLimit ? (
            <div className="px-4 pt-2">
              <p className="type-footnote text-label-secondary">Found on this computer</p>
              <ul className="mt-1 space-y-1">
                {claudeProfiles.suggestions.map((suggestion) => (
                  <li key={suggestion.path} className="flex items-center gap-2">
                    <span className="min-w-0 flex-1">
                      <PathText path={suggestion.path} />
                    </span>
                    <PushButton
                      className="shrink-0 gap-1.5"
                      ariaLabel={`Add ${suggestion.path} as a Claude profile`}
                      onClick={() => openEditor({ kind: "add", suggestion })}
                    >
                      <Plus size={12} aria-hidden="true" /> Add
                    </PushButton>
                  </li>
                ))}
              </ul>
            </div>
          ) : null}
          <div className="flex flex-wrap items-center gap-3 px-4 py-3">
            <PushButton
              id="add-claude-profile"
              className="shrink-0 gap-1.5"
              disabled={atLimit}
              onClick={() => openEditor({ kind: "add", suggestion: null })}
            >
              <Plus size={12} aria-hidden="true" /> Add profile…
            </PushButton>
            {atLimit ? (
              <p className="type-footnote text-label-tertiary">
                {claudeProfiles.maxProfiles}-profile limit reached.
              </p>
            ) : null}
          </div>
        </Card>
      </SettingsSectionGroup>
      {editor ? (
        <ClaudeProfileEditor
          key={editor.target.kind === "rename" ? editor.target.profile.id : "new"}
          target={editor.target}
          session={session}
          maxLabelChars={claudeProfiles.maxLabelChars}
          onClose={() => {
            const target = editor.restoreFocus
            setEditor(null)
            restoreFocus(target)
          }}
        />
      ) : null}
      {removing ? (
        <RemoveClaudeProfileDialog
          profile={removing.profile}
          session={session}
          onClose={() => {
            const target = removing.restoreFocus
            setRemoving(null)
            restoreFocus(target)
          }}
        />
      ) : null}
    </>
  )
}
