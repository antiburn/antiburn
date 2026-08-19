// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { fireEvent, render } from "@testing-library/react"
import { describe, expect, it } from "vitest"

import { TextRoll } from "./TextRoll"

// jsdom parses but does not run CSS animations, so these tests assert the
// structure the stylesheet animates: which cells carry roll faces, what the
// faces contain, and what assistive tech sees.

function cells(container: HTMLElement): HTMLElement[] {
  return Array.from(container.querySelectorAll(".text-roll-cell"))
}

function srText(container: HTMLElement): string {
  return container.querySelector(".text-roll-sr")?.textContent ?? ""
}

function rollingFaces(container: HTMLElement): HTMLElement[] {
  return Array.from(container.querySelectorAll(".text-roll-in, .text-roll-out"))
}

describe("TextRoll", () => {
  it("renders one cell per character with the full text for assistive tech", () => {
    const { container } = render(<TextRoll text="$0.42" />)

    expect(srText(container)).toBe("$0.42")
    expect(cells(container).map((cell) => cell.textContent)).toEqual(["$", "0", ".", "4", "2"])
    expect(container.querySelector(".text-roll-cells")?.getAttribute("aria-hidden")).toBe(
      "true",
    )
    expect(rollingFaces(container)).toHaveLength(0)
  })

  it("does not animate on a re-render with identical text", () => {
    const { container, rerender } = render(<TextRoll text="$0.42" />)

    rerender(<TextRoll text="$0.42" />)

    expect(rollingFaces(container)).toHaveLength(0)
    expect(srText(container)).toBe("$0.42")
  })

  it("rolls only the characters that changed", () => {
    const { container, rerender } = render(<TextRoll text="$0.42" />)

    rerender(<TextRoll text="$0.57" />)

    const allCells = cells(container)
    // "$0." prefix is untouched.
    for (const cell of allCells.slice(0, 3)) {
      expect(cell.querySelector(".text-roll-in")).toBeNull()
      expect(cell.querySelector(".text-roll-out")).toBeNull()
    }
    // "42" -> "57": old glyph rolls out, new glyph rolls in.
    expect(allCells[3]!.querySelector(".text-roll-out")?.textContent).toBe("4")
    expect(allCells[3]!.querySelector(".text-roll-in")?.textContent).toBe("5")
    expect(allCells[4]!.querySelector(".text-roll-out")?.textContent).toBe("2")
    expect(allCells[4]!.querySelector(".text-roll-in")?.textContent).toBe("7")
    expect(srText(container)).toBe("$0.57")
  })

  it("aligns by the right edge when the text grows, keeping punctuation still", () => {
    const { container, rerender } = render(<TextRoll text="$9.99" />)

    rerender(<TextRoll text="$10.00" />)

    const allCells = cells(container)
    expect(allCells).toHaveLength(6)
    // The new leading cell rolls in with nothing to roll out.
    expect(allCells[0]!.querySelector(".text-roll-in")?.textContent).toBe("$")
    expect(allCells[0]!.querySelector(".text-roll-out")).toBeNull()
    // "$"→"1" and "9"→"0" roll; the decimal point never becomes a digit.
    expect(allCells[1]!.querySelector(".text-roll-out")?.textContent).toBe("$")
    expect(allCells[2]!.querySelector(".text-roll-out")?.textContent).toBe("9")
    expect(allCells[3]!.querySelector(".text-roll-in")).toBeNull()
    expect(allCells[3]!.querySelector(".text-roll-out")).toBeNull()
    expect(allCells[3]!.textContent).toBe(".")
    expect(allCells[4]!.querySelector(".text-roll-out")?.textContent).toBe("9")
    expect(allCells[5]!.querySelector(".text-roll-out")?.textContent).toBe("9")
    expect(srText(container)).toBe("$10.00")
  })

  it("drops cells when the text shrinks", () => {
    const { container, rerender } = render(<TextRoll text="$10.00" />)

    rerender(<TextRoll text="$9.99" />)

    expect(cells(container)).toHaveLength(5)
    expect(srText(container)).toBe("$9.99")
  })

  it("re-targets an interrupted roll to the newest value", () => {
    const { container, rerender } = render(<TextRoll text="$0.42" />)

    rerender(<TextRoll text="$0.57" />)
    rerender(<TextRoll text="$0.61" />)

    const allCells = cells(container)
    // The outgoing face shows the interrupted roll's target, not the original.
    expect(allCells[3]!.querySelector(".text-roll-out")?.textContent).toBe("5")
    expect(allCells[3]!.querySelector(".text-roll-in")?.textContent).toBe("6")
    expect(allCells[4]!.querySelector(".text-roll-out")?.textContent).toBe("7")
    expect(allCells[4]!.querySelector(".text-roll-in")?.textContent).toBe("1")
    expect(srText(container)).toBe("$0.61")
  })

  it("settles a cell that matches the newest value mid-roll", () => {
    const { container, rerender } = render(<TextRoll text="$0.42" />)

    rerender(<TextRoll text="$0.57" />)
    rerender(<TextRoll text="$0.51" />)

    const allCells = cells(container)
    // Cell 3 already shows "5": it snaps to rest with no roll faces.
    expect(allCells[3]!.querySelector(".text-roll-in")).toBeNull()
    expect(allCells[3]!.querySelector(".text-roll-out")).toBeNull()
    expect(allCells[3]!.textContent).toBe("5")
    expect(allCells[4]!.querySelector(".text-roll-in")?.textContent).toBe("1")
  })

  it("handles a leading sentinel character with a minimal roll", () => {
    const { container, rerender } = render(<TextRoll text="$0.00" />)

    rerender(<TextRoll text="<$0.01" />)

    const allCells = cells(container)
    expect(allCells).toHaveLength(6)
    // Right-aligned: only "<" rolls in up front and the last digit ticks;
    // "$0.0" stays perfectly still.
    expect(allCells[0]!.querySelector(".text-roll-in")?.textContent).toBe("<")
    expect(allCells[0]!.querySelector(".text-roll-out")).toBeNull()
    for (const cell of allCells.slice(1, 5)) {
      expect(cell.querySelector(".text-roll-in")).toBeNull()
      expect(cell.querySelector(".text-roll-out")).toBeNull()
    }
    expect(allCells[5]!.querySelector(".text-roll-out")?.textContent).toBe("0")
    expect(allCells[5]!.querySelector(".text-roll-in")?.textContent).toBe("1")
    expect(srText(container)).toBe("<$0.01")
  })

  it("collapses the roll structure when the last animation ends", () => {
    const { container, rerender } = render(<TextRoll text="$0.42" />)

    rerender(<TextRoll text="$0.57" />)
    expect(rollingFaces(container).length).toBeGreaterThan(0)

    // Only the last-finishing cell's incoming face carries the settle handler;
    // dispatching on every in-face reaches it without knowing which it is.
    // jsdom lacks style.animation, so React's vendor-prefix detection registers
    // webkitAnimationEnd here (browsers use the unprefixed name) — dispatch
    // both; the settle handler is idempotent.
    for (const face of Array.from(container.querySelectorAll(".text-roll-in"))) {
      fireEvent(face, new Event("animationend", { bubbles: true }))
      fireEvent(face, new Event("webkitAnimationEnd", { bubbles: true }))
    }

    expect(rollingFaces(container)).toHaveLength(0)
    expect(srText(container)).toBe("$0.57")
    expect(cells(container).map((cell) => cell.textContent)).toEqual(["$", "0", ".", "5", "7"])
  })

  it("sets per-cell motion variables on changed cells only", () => {
    const { container, rerender } = render(<TextRoll text="$0.42" />)

    rerender(<TextRoll text="$0.43" />)

    const allCells = cells(container)
    expect(allCells[0]!.style.getPropertyValue("--tr-sp")).toBe("")
    const changedCell = allCells[4]!
    expect(changedCell.style.getPropertyValue("--tr-sp")).not.toBe("")
    expect(changedCell.style.getPropertyValue("--tr-dm")).not.toBe("")
    expect(changedCell.style.getPropertyValue("--tr-tilt")).toMatch(/deg$/)
  })

  it("merges a custom className onto the root", () => {
    const { container } = render(<TextRoll text="$0.42" className="pill" />)

    const root = container.querySelector(".text-roll")
    expect(root?.classList.contains("pill")).toBe(true)
  })
})
