import { ArrowLeft, ArrowRight, Search } from "lucide-react"
import type { ReactNode } from "react"
import { detectPlatform, isMacOS } from "../../lib/platform"
import { useGlobalKeydown } from "../../lib/useGlobalKeydown"
import { Tooltip } from "../../components/presentation/Tooltip"
import { WindowResizeHandles } from "./WindowResizeHandles"
import { WindowControls } from "./WindowControls"
import { dragViewHeader, finishViewHeaderClick } from "./viewHeaderDrag"

export function MainWindowLayout({
  sidebar,
  children,
  canBack = false,
  canForward = false,
  onBack,
  onForward,
  onSearch,
  searchOpen = false,
}: {
  sidebar: ReactNode
  children: ReactNode
  canBack?: boolean
  canForward?: boolean
  onBack?: () => void
  onForward?: () => void
  onSearch?: () => void
  searchOpen?: boolean
}) {
  const mac = isMacOS()
  const command = mac ? "⌘" : "Ctrl+"
  function openSearch() {
    onSearch?.()
  }
  useGlobalKeydown(true, (event) => {
    if (event.defaultPrevented || event.repeat || event.isComposing) return
    if (searchOpen || document.querySelector("dialog[open], [aria-modal='true']")) return
    const target = event.target
    if (
      target instanceof HTMLElement &&
      target.closest(
        "input, textarea, select, [contenteditable]:not([contenteditable='false'])",
      )
    )
      return
    const modifier = mac ? event.metaKey && !event.ctrlKey : event.ctrlKey && !event.metaKey
    if (modifier && !event.altKey && !event.shiftKey && event.key.toLowerCase() === "k") {
      event.preventDefault()
      openSearch()
      return
    }
    const back = mac
      ? modifier && event.key === "[" && !event.altKey
      : event.altKey && event.key === "ArrowLeft" && !event.ctrlKey && !event.metaKey
    const forward = mac
      ? modifier && event.key === "]" && !event.altKey
      : event.altKey && event.key === "ArrowRight" && !event.ctrlKey && !event.metaKey
    if (!event.shiftKey && (back || forward)) {
      event.preventDefault()
      if (back && canBack) onBack?.()
      if (forward && canForward) onForward?.()
    }
  })
  const searchButton = () => (
    <Tooltip label={`Search antiburn · ${command}K`}>
      <button
        type="button"
        className="main-window-tool"
        data-app-search-trigger="titlebar"
        aria-label="Search antiburn"
        aria-haspopup="dialog"
        aria-expanded={searchOpen}
        onClick={openSearch}
      >
        <Search size={14} aria-hidden="true" className="shrink-0" />
      </button>
    </Tooltip>
  )
  return (
    <main
      className={`main-window${mac ? " main-window-macos" : ""}`}
      aria-label="antiburn main window"
    >
      {detectPlatform() === "linux" && <WindowResizeHandles />}
      <header className="main-window-titlebar" data-tauri-drag-region>
        <div className="main-window-titlebar-controls" data-tauri-drag-region>
          <Tooltip label={`Back · ${mac ? "⌘[" : "Alt+←"}`}>
            <button
              type="button"
              className="main-window-tool"
              aria-label="Back"
              disabled={!canBack}
              onClick={() => {
                onBack?.()
              }}
            >
              <ArrowLeft size={16} aria-hidden="true" />
            </button>
          </Tooltip>
          <Tooltip label={`Forward · ${mac ? "⌘]" : "Alt+→"}`}>
            <button
              type="button"
              className="main-window-tool"
              aria-label="Forward"
              disabled={!canForward}
              onClick={() => {
                onForward?.()
              }}
            >
              <ArrowRight size={16} aria-hidden="true" />
            </button>
          </Tooltip>
          {onSearch && searchButton()}
        </div>
        <div className="main-window-drag-space" data-tauri-drag-region />
        {!mac && <WindowControls />}
      </header>
      <div className="main-window-body">
        <div id="main-sidebar" className="main-window-navigation">
          {sidebar}
        </div>
        <div
          tabIndex={-1}
          className="main-window-workspace"
          onMouseDown={dragViewHeader}
          onMouseUp={finishViewHeaderClick}
        >
          {children}
        </div>
      </div>
    </main>
  )
}
