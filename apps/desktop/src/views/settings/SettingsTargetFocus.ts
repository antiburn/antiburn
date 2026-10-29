/** Reveal each explicit search target once, including controls that load later. */
export class SettingsTargetFocus {
  private applied: string | null = null
  private recoveryKey: string | null = null
  private recoveryRow: HTMLElement | null = null
  private dismissed: string | null = null
  private attachment: {
    key: string
    node: HTMLDivElement
    stop: () => void
  } | null = null

  attach(
    node: HTMLDivElement,
    pane: string,
    control: string | null | undefined,
    revision: number | undefined,
  ): () => void {
    const key = `${pane}:${control ?? ""}:${revision ?? 0}`
    if (this.attachment?.key === key && this.attachment.node === node) return () => undefined
    this.attachment?.stop()
    if (this.applied === key || this.dismissed === key) return () => undefined
    const document = node.ownerDocument
    let targetRow: HTMLElement | undefined
    let blockingModal: Element | null = null
    const reveal = () => {
      if (!control) {
        const viewport = node.closest<HTMLDivElement>(".ui-scroll-viewport")
        if (viewport) viewport.scrollTop = 0
        this.applied = key
        return true
      }
      const row = [...node.querySelectorAll<HTMLElement>("[data-settings-control]")].find(
        (element) => element.dataset.settingsControl === control,
      )
      if (!row) return false
      targetRow = row
      const modal = document.querySelector("dialog[open]")
      blockingModal = modal && !modal.contains(row) ? modal : null
      if (blockingModal) return false
      if (row.closest("[inert], [hidden], [aria-hidden=true]")) return false
      if (row.closest('[aria-busy="true"]') || row.querySelector('[aria-busy="true"]'))
        return false
      const pending = row.hasAttribute("data-settings-target-pending")
      if (pending && this.recoveryKey === key && this.recoveryRow === row) return false
      row.scrollIntoView({ block: "center", behavior: "instant" })
      const field = row.hasAttribute("data-settings-focus-self")
        ? null
        : row.querySelector<HTMLElement>(
            "button:not(:disabled):not([aria-disabled=true]), input:not(:disabled):not([aria-disabled=true]), select:not(:disabled):not([aria-disabled=true]), [role=switch]:not(:disabled):not([aria-disabled=true])",
          )
      const target = field ?? row
      target.focus({ preventScroll: true })
      if (document.activeElement !== target) return false
      if (pending) {
        this.recoveryKey = key
        this.recoveryRow = row
        return false
      }
      this.applied = key
      this.recoveryKey = null
      this.recoveryRow = null
      return true
    }
    if (reveal()) return () => undefined
    const observer = new MutationObserver(() => {
      if (reveal()) stop()
      else observe()
    })
    const dismiss = (event: Event) => {
      if (document.querySelector("dialog[open]")) return
      if (event.target instanceof Node && targetRow?.contains(event.target)) return
      this.dismissed = key
      stop()
    }
    const stop = () => {
      observer.disconnect()
      document.removeEventListener("pointerdown", dismiss, true)
      document.removeEventListener("keydown", dismiss, true)
      if (this.attachment?.stop === stop) this.attachment = null
    }
    this.attachment = { key, node, stop }
    document.addEventListener("pointerdown", dismiss, true)
    document.addEventListener("keydown", dismiss, true)
    const observe = () => {
      observer.disconnect()
      observer.observe(node, {
        childList: true,
        subtree: true,
        attributes: true,
        attributeFilter: [
          "aria-busy",
          "disabled",
          "aria-disabled",
          "data-settings-target-pending",
          "open",
          "inert",
          "hidden",
          "aria-hidden",
        ],
      })
      if (blockingModal && !node.contains(blockingModal)) {
        observer.observe(blockingModal, { attributes: true, attributeFilter: ["open"] })
        if (blockingModal.parentNode)
          observer.observe(blockingModal.parentNode, { childList: true })
      }
    }
    observe()
    return stop
  }
}
