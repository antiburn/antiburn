import { Check, Copy, Folder, FolderOpen } from "lucide-react"
import { Fragment, useCallback, useId, useRef, useState } from "react"
import { createPortal } from "react-dom"

import { detectPlatform } from "../../lib/platform"
import { Tooltip } from "../presentation/Tooltip"

interface ProjectFolderActionsProps {
  path: string
  onOpen: () => Promise<void>
  onCopy: () => Promise<void>
}

/** Show the project path and its actions on deliberate hover or keyboard focus. */
export function ProjectFolderActions({ path, onOpen, onCopy }: ProjectFolderActionsProps) {
  const id = useId()
  const trigger = useRef<HTMLButtonElement>(null)
  const panel = useRef<HTMLDivElement>(null)
  const live = useRef(false)
  const busy = useRef(false)
  const pointer = useRef({ trigger: false, panel: false })
  const suppressFocus = useRef(false)
  const openTimer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined)
  const closeTimer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined)
  const copiedTimer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined)
  const [open, setOpen] = useState(false)
  const [copied, setCopied] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const platform = detectPlatform()
  const openLabel =
    platform === "macos"
      ? "Open in Finder"
      : platform === "windows"
        ? "Open in File Explorer"
        : "Open in file manager"
  const segments = path.match(/[^/\\]+[/\\]?|[/\\]/g) ?? [path]

  const cancelHover = useCallback(() => {
    clearTimeout(openTimer.current)
    clearTimeout(closeTimer.current)
  }, [])

  const hide = useCallback((restore = false) => {
    clearTimeout(openTimer.current)
    clearTimeout(closeTimer.current)
    pointer.current.panel = false
    setOpen(false)
    if (restore) {
      suppressFocus.current = true
      trigger.current?.focus()
      suppressFocus.current = false
    }
  }, [])

  const bindLifetime = useCallback((node: HTMLSpanElement | null) => {
    if (!node) return
    live.current = true
    return () => {
      live.current = false
      clearTimeout(openTimer.current)
      clearTimeout(closeTimer.current)
      clearTimeout(copiedTimer.current)
    }
  }, [])

  function show() {
    cancelHover()
    setOpen(true)
  }

  function scheduleClose() {
    cancelHover()
    closeTimer.current = setTimeout(() => {
      const focused = document.activeElement
      if (
        !pointer.current.trigger &&
        !pointer.current.panel &&
        focused !== trigger.current &&
        !panel.current?.contains(focused)
      ) {
        hide()
      }
    }, 200)
  }

  const bindPanel = useCallback(
    (node: HTMLDivElement | null) => {
      panel.current = node
      if (!node || !trigger.current) return
      const position = () => {
        if (!trigger.current) return
        const anchor = trigger.current.getBoundingClientRect()
        const bounds = node.getBoundingClientRect()
        const pane = trigger.current.closest("[data-detail-pane]")?.getBoundingClientRect()
        const gutter = 8
        const minimumLeft =
          pane && pane.width >= bounds.width + gutter * 2 ? pane.left + gutter : gutter
        node.style.left = `${Math.max(minimumLeft, Math.min(anchor.right - bounds.width, window.innerWidth - bounds.width - gutter))}px`
        const below = anchor.bottom + gutter
        node.style.top = `${Math.max(gutter, below + bounds.height <= window.innerHeight - gutter ? below : anchor.top - bounds.height - gutter)}px`
      }
      position()
      const observer =
        typeof ResizeObserver === "undefined" ? null : new ResizeObserver(position)
      observer?.observe(node)

      const onKeyDown = (event: KeyboardEvent) => {
        if (event.key === "Escape") {
          event.preventDefault()
          event.stopImmediatePropagation()
          hide(node.contains(document.activeElement))
        } else if (
          ((event.key === "Tab" && !event.shiftKey) || event.key === "ArrowDown") &&
          document.activeElement === trigger.current
        ) {
          event.preventDefault()
          node.querySelector<HTMLButtonElement>("button")?.focus()
        } else if (
          event.key === "Tab" &&
          event.shiftKey &&
          document.activeElement === node.querySelector("button")
        ) {
          event.preventDefault()
          trigger.current?.focus()
        }
      }
      const onOutside = (event: PointerEvent) => {
        if (
          event.target instanceof Node &&
          !node.contains(event.target) &&
          !trigger.current?.contains(event.target)
        )
          hide()
      }
      const onScroll = (event: Event) => {
        if (!(event.target instanceof Node) || !node.contains(event.target)) hide()
      }
      const onResize = () => hide()
      document.addEventListener("keydown", onKeyDown, true)
      document.addEventListener("pointerdown", onOutside, true)
      document.addEventListener("scroll", onScroll, true)
      window.addEventListener("resize", onResize)
      window.addEventListener("blur", onResize)
      return () => {
        observer?.disconnect()
        panel.current = null
        document.removeEventListener("keydown", onKeyDown, true)
        document.removeEventListener("pointerdown", onOutside, true)
        document.removeEventListener("scroll", onScroll, true)
        window.removeEventListener("resize", onResize)
        window.removeEventListener("blur", onResize)
      }
    },
    [hide],
  )

  async function run(action: "open" | "copy") {
    if (busy.current) return
    busy.current = true
    setError(null)
    try {
      await (action === "open" ? onOpen() : onCopy())
      if (!live.current) return
      if (action === "copy") {
        setCopied(true)
        clearTimeout(copiedTimer.current)
        copiedTimer.current = setTimeout(() => setCopied(false), 2_000)
      } else {
        hide(panel.current?.contains(document.activeElement))
      }
    } catch {
      if (live.current)
        setError(
          action === "copy"
            ? "Couldn’t copy the path. Try again."
            : "Couldn’t open this folder. It may have moved or become unavailable.",
        )
    } finally {
      busy.current = false
    }
  }

  return (
    <span ref={bindLifetime} className="inline-flex shrink-0">
      <button
        ref={trigger}
        type="button"
        className="project-folder-icon text-label-tertiary hover:text-label"
        aria-label="Project folder"
        aria-haspopup="dialog"
        aria-expanded={open}
        aria-controls={open ? id : undefined}
        onPointerEnter={(event) => {
          if (event.pointerType === "touch") return
          pointer.current.trigger = true
          cancelHover()
          if (!open) openTimer.current = setTimeout(show, 300)
        }}
        onPointerLeave={() => {
          pointer.current.trigger = false
          scheduleClose()
        }}
        onPointerDown={(event) => {
          if (event.pointerType === "mouse") event.preventDefault()
          if (event.pointerType === "touch") show()
        }}
        onFocus={() => {
          if (!suppressFocus.current) show()
        }}
        onBlur={scheduleClose}
        onKeyDown={(event) => {
          if (event.key === "Escape") {
            event.preventDefault()
            event.stopPropagation()
            hide()
          }
        }}
      >
        <Folder size={14} aria-hidden="true" />
      </button>
      {open &&
        createPortal(
          <div
            ref={bindPanel}
            id={id}
            role="dialog"
            aria-modal="false"
            aria-labelledby={`${id}-title`}
            className="ui-menu project-folder-panel rounded-popover text-label"
            onPointerEnter={() => {
              pointer.current.panel = true
              cancelHover()
            }}
            onPointerLeave={() => {
              pointer.current.panel = false
              scheduleClose()
            }}
            onFocusCapture={cancelHover}
            onBlurCapture={scheduleClose}
          >
            <div className="project-folder-heading">
              <span id={`${id}-title`} className="type-body font-semibold!">
                Project folder
              </span>
              <div className="project-folder-tools">
                <Tooltip label={openLabel}>
                  <button
                    type="button"
                    className="project-folder-icon text-label-secondary hover:text-label"
                    aria-label={openLabel}
                    onClick={() => void run("open")}
                  >
                    <FolderOpen size={14} aria-hidden="true" />
                  </button>
                </Tooltip>
                <Tooltip label={copied ? "Copied" : "Copy path"}>
                  <button
                    type="button"
                    className="project-folder-icon text-label-secondary hover:text-label"
                    aria-label={copied ? "Path copied" : "Copy path"}
                    onClick={() => void run("copy")}
                  >
                    {copied ? (
                      <Check size={14} aria-hidden="true" />
                    ) : (
                      <Copy size={14} aria-hidden="true" />
                    )}
                  </button>
                </Tooltip>
              </div>
            </div>
            <div className="project-folder-path type-callout font-mono text-label-secondary">
              {segments.map((segment, index) => (
                <Fragment key={index}>
                  <span className={index === segments.length - 1 ? "text-label" : undefined}>
                    {segment}
                  </span>
                  {index < segments.length - 1 && <wbr />}
                </Fragment>
              ))}
            </div>
            {error && (
              <p className="type-callout text-system-red-text mt-2" role="alert">
                {error}
              </p>
            )}
            <span className="sr-only" role="status" aria-live="polite">
              {copied ? "Project folder path copied." : ""}
            </span>
          </div>,
          document.body,
        )}
    </span>
  )
}
