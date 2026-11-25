/** Restore a usable trigger, replacement control, or list only if the action removed focus. */
export async function changeFolderWithFocus(
  trigger: Element | null,
  foldersId: string,
  folderId: string | undefined,
  change: () => Promise<void>,
  fallbackControl?: string,
) {
  if (!trigger || document.activeElement !== trigger) return change()

  let focusMoved = false
  const onFocus = (event: FocusEvent) => {
    if (event.target !== trigger && event.target !== document.body) focusMoved = true
  }
  const onBlur = () => {
    focusMoved = true
  }
  document.addEventListener("focusin", onFocus)
  window.addEventListener("blur", onBlur)
  try {
    await change()
    await new Promise<void>((resolve) => window.requestAnimationFrame(() => resolve()))
    if (
      focusMoved ||
      (document.activeElement !== document.body &&
        !(document.activeElement === trigger && trigger.matches(":disabled")))
    )
      return
    if (
      trigger instanceof HTMLElement &&
      trigger.isConnected &&
      !trigger.matches(':disabled, [aria-disabled="true"]')
    ) {
      trigger.focus()
      return
    }
    const folders = document.getElementById(foldersId)
    const row = Array.from(
      folders?.querySelectorAll<HTMLElement>("[data-cloud-folder]") ?? [],
    ).find((row) => row.dataset.cloudFolder === folderId)
    const control = row?.querySelector<HTMLElement>(
      'select:not(:disabled), button[role="switch"]:not(:disabled)',
    )
    const target =
      control ??
      (fallbackControl ? folders?.querySelector<HTMLElement>(fallbackControl) : null) ??
      folders
    target?.focus()
  } finally {
    document.removeEventListener("focusin", onFocus)
    window.removeEventListener("blur", onBlur)
  }
}
