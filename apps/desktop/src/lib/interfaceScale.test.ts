import { describe, expect, it } from "vitest"

import { interfaceScaleShortcut } from "./interfaceScale"

describe("interface scale shortcuts", () => {
  it.each(["+", "="])("accepts the %s key with the platform modifier", (key) => {
    expect(
      interfaceScaleShortcut(new KeyboardEvent("keydown", { key, ctrlKey: true }), false),
    ).toEqual({ kind: "increase" })
    expect(
      interfaceScaleShortcut(new KeyboardEvent("keydown", { key, metaKey: true }), true),
    ).toEqual({ kind: "increase" })
  })

  it.each([
    ["-", "", "decrease"],
    ["0", "", "reset"],
    ["Add", "NumpadAdd", "increase"],
    ["Subtract", "NumpadSubtract", "decrease"],
    ["0", "Numpad0", "reset"],
  ])("accepts %s %s", (key, code, kind) => {
    expect(
      interfaceScaleShortcut(new KeyboardEvent("keydown", { key, code, ctrlKey: true }), false),
    ).toEqual({ kind })
  })

  it.each([
    { key: "+" },
    { key: "+", metaKey: true },
    { key: "+", ctrlKey: true, altKey: true },
    { key: "+", ctrlKey: true, isComposing: true },
    { key: "0", ctrlKey: true, shiftKey: true },
    { key: "a", ctrlKey: true },
  ])("leaves unrelated input alone: %j", (init) => {
    expect(interfaceScaleShortcut(new KeyboardEvent("keydown", init), false)).toBeNull()
  })
})
