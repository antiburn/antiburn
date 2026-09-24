import { slowAnimationDurationMs } from "../../lib/popoverHeight"

export function observeMenuPointerExit({
  content,
  trigger,
  pointerOpened,
  onDismiss,
}: {
  content: HTMLElement
  trigger: () => HTMLElement | null
  pointerOpened: boolean
  onDismiss: () => void
}): () => void {
  const document = content.ownerDocument
  const styles = getComputedStyle(content)
  const grace = Number.parseFloat(styles.getPropertyValue("--space-sm")) || 0
  const delay = slowAnimationDurationMs()
  let pointerActive = pointerOpened
  let timer: ReturnType<typeof setTimeout> | undefined

  function cancel() {
    clearTimeout(timer)
    timer = undefined
  }

  function schedule() {
    if (!pointerActive || timer !== undefined) return
    timer = setTimeout(() => {
      timer = undefined
      onDismiss()
    }, delay)
  }

  function containsPointer(element: HTMLElement | null, event: PointerEvent): boolean {
    if (!element) return false
    const rect = element.getBoundingClientRect()
    return (
      event.clientX >= rect.left - grace &&
      event.clientX <= rect.right + grace &&
      event.clientY >= rect.top - grace &&
      event.clientY <= rect.bottom + grace
    )
  }

  function move(event: PointerEvent) {
    if (event.pointerType !== "mouse") return
    // Keep the menu open while the reader moves into its tooltip.
    const descriptions = [...content.querySelectorAll("[aria-describedby]")].flatMap((item) =>
      (item.getAttribute("aria-describedby") ?? "").split(/\s+/),
    )
    const inside =
      containsPointer(content, event) ||
      containsPointer(trigger(), event) ||
      descriptions.some((id) => {
        const description = document.getElementById(id)
        return containsPointer(
          description?.closest<HTMLElement>("[data-radix-popper-content-wrapper]") ??
            description,
          event,
        )
      })
    if (inside) {
      pointerActive = true
      cancel()
    } else {
      schedule()
    }
  }

  function leaveWindow(event: PointerEvent) {
    if (event.pointerType === "mouse" && event.relatedTarget === null) schedule()
  }

  function cancelPointerDismissal() {
    pointerActive = false
    cancel()
  }

  function handlePointerDown(event: PointerEvent) {
    if (event.pointerType !== "mouse") cancelPointerDismissal()
  }

  document.addEventListener("pointermove", move)
  document.addEventListener("pointerout", leaveWindow)
  document.addEventListener("keydown", cancelPointerDismissal, true)
  document.addEventListener("pointerdown", handlePointerDown, true)
  return () => {
    cancel()
    document.removeEventListener("pointermove", move)
    document.removeEventListener("pointerout", leaveWindow)
    document.removeEventListener("keydown", cancelPointerDismissal, true)
    document.removeEventListener("pointerdown", handlePointerDown, true)
  }
}
