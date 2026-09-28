import { describe, expect, it } from "vitest"
import { searchEvidencePreview, searchEvidenceHighlights } from "./searchEvidencePreview"

describe("searchEvidencePreview", () => {
  it("shows matching prose after a long path introduction", () => {
    const source =
      "In the repo at /Users/example/antiburn/focus-stealing, find every place that could cause the desktop app window to steal focus unexpectedly."
    const preview = searchEvidencePreview(source, "antiburn focus stealing")
    expect(preview).toBe("…the desktop app window to steal focus unexpectedly.")
    expect(source.endsWith(preview.slice(1))).toBe(true)
  })

  it("preserves explicit paths and short source passages", () => {
    const source = "  Open  /Users/example/repo to inspect focus behavior. "
    expect(searchEvidencePreview(source, "/Users/example/repo")).toBe(
      "Open  /Users/example/repo to inspect focus behavior.",
    )
    expect(searchEvidencePreview("Preserve window focus.", "focus")).toBe(
      "Preserve window focus.",
    )
  })

  it("leaves unmatched evidence intact", () => {
    expect(searchEvidencePreview("Different source text.", "window")).toBe(
      "Different source text.",
    )
  })

  it("preserves whitespace inside source content", () => {
    const source = "printf 'a  b'\n\tthen inspect focus"
    expect(searchEvidencePreview(source, "focus")).toBe(source)
  })
})

describe("searchEvidenceHighlights", () => {
  it.each(["C++", "[a]", "a.b", "a\\b", "日本語", "<script>"])(
    "preserves inert text and literal punctuation: %s",
    (query) => {
      const text = `Before ${query} after`
      const parts = searchEvidenceHighlights(text, query)
      expect(parts.map((part) => part.text).join("")).toBe(text)
      expect(
        parts
          .filter((part) => part.match)
          .map((part) => part.text)
          .join(""),
      ).toBe(query)
    },
  )
  it("highlights terms case-insensitively without changing source text", () => {
    const parts = searchEvidenceHighlights("Focus remains in the WINDOW", "focus window")
    expect(parts.filter((part) => part.match).map((part) => part.text)).toEqual([
      "Focus",
      "WINDOW",
    ])
  })
})
