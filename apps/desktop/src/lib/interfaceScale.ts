import presets from "../../interface-scale.json"

export const INTERFACE_SCALE_PRESETS: readonly number[] = presets
export const DEFAULT_INTERFACE_SCALE_PERCENT = 100

export type InterfaceScaleChange =
  | { kind: "set"; percent: number }
  | { kind: "increase" }
  | { kind: "decrease" }
  | { kind: "reset" }

export type InterfaceScaleSource = "settings" | "shortcut" | "menu"

/** Match application shortcuts without consuming text composition or alternate modifiers. */
export function interfaceScaleShortcut(
  event: Pick<
    KeyboardEvent,
    "key" | "code" | "metaKey" | "ctrlKey" | "altKey" | "shiftKey" | "isComposing"
  >,
  macOS: boolean,
): InterfaceScaleChange | null {
  if (event.isComposing || event.altKey) return null
  if (macOS ? !event.metaKey || event.ctrlKey : !event.ctrlKey || event.metaKey) return null
  if (event.key === "+" || event.key === "=" || event.code === "NumpadAdd")
    return { kind: "increase" }
  if (event.key === "-" || event.code === "NumpadSubtract") return { kind: "decrease" }
  if (!event.shiftKey && (event.key === "0" || event.code === "Numpad0"))
    return { kind: "reset" }
  return null
}
