import { useCallback, useState } from "react"

export function hasDiscussionModifier(event: {
  altKey: boolean
  metaKey: boolean
  ctrlKey: boolean
}): boolean {
  return event.altKey || event.metaKey || event.ctrlKey
}

/** Track modifier keys at the toolbar's DOM boundary. */
export function useDiscussionModifiers(enabled: boolean, sessionKey: string) {
  const [state, setState] = useState({ sessionKey, modified: false })
  const bindModifiers = useCallback(
    (node: HTMLDivElement | null) => {
      if (!node || !enabled) return
      let hovered = false
      let focused = node.contains(document.activeElement)
      let held = false
      const update = () => {
        const modified = (hovered || focused) && held
        setState((previous) =>
          previous.sessionKey === sessionKey && previous.modified === modified
            ? previous
            : { sessionKey, modified },
        )
      }
      const key = (event: KeyboardEvent) => {
        held = hasDiscussionModifier(event)
        update()
      }
      const enter = (event: MouseEvent) => {
        hovered = true
        held = hasDiscussionModifier(event)
        update()
      }
      const leave = () => {
        hovered = false
        update()
      }
      const focus = () => {
        focused = true
        update()
      }
      const blur = (event: FocusEvent) => {
        focused = event.relatedTarget instanceof Node && node.contains(event.relatedTarget)
        update()
      }
      const reset = () => {
        hovered = false
        focused = false
        held = false
        update()
      }
      window.addEventListener("keydown", key)
      window.addEventListener("keyup", key)
      window.addEventListener("blur", reset)
      node.addEventListener("mouseenter", enter)
      node.addEventListener("mousemove", enter)
      node.addEventListener("mouseleave", leave)
      node.addEventListener("focusin", focus)
      node.addEventListener("focusout", blur)
      return () => {
        window.removeEventListener("keydown", key)
        window.removeEventListener("keyup", key)
        window.removeEventListener("blur", reset)
        node.removeEventListener("mouseenter", enter)
        node.removeEventListener("mousemove", enter)
        node.removeEventListener("mouseleave", leave)
        node.removeEventListener("focusin", focus)
        node.removeEventListener("focusout", blur)
        reset()
      }
    },
    [enabled, sessionKey],
  )
  return {
    bindModifiers,
    modified: enabled && state.sessionKey === sessionKey && state.modified,
  }
}
