export function searchEvidencePreview(excerpt: string, query: string): string {
  const normalized = excerpt.trim()
  if (/[\\/]/u.test(query)) return normalized

  const terms = new Set(query.toLowerCase().match(/[\p{L}\p{N}_-]{3,}/gu) ?? [])
  const words = Array.from(normalized.matchAll(/\S+/gu))
  const anchor = words.findIndex(([word]) => {
    if (/[\\/]/u.test(word)) return false
    return (word.toLowerCase().match(/[\p{L}\p{N}_-]+/gu) ?? []).some((term) => terms.has(term))
  })
  if (anchor < 12) return normalized

  const start = words[anchor - 6]?.index ?? 0
  return start > 0 ? `…${normalized.slice(start)}` : normalized
}

export function searchEvidenceHighlights(
  text: string,
  query: string,
): { text: string; match: boolean }[] {
  const phrase = query.trim().replace(/^"|"$/g, "")
  const terms = [...new Set([phrase, ...(phrase.match(/[\p{L}\p{N}_-]{2,}/gu) ?? [])])]
    .filter(Boolean)
    .sort((a, b) => b.length - a.length)
  if (!terms.length) return [{ text, match: false }]
  const expression = new RegExp(
    terms.map((term) => term.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")).join("|"),
    "giu",
  )
  const parts: { text: string; match: boolean }[] = []
  let cursor = 0
  for (const match of text.matchAll(expression)) {
    const index = match.index
    if (index > cursor) parts.push({ text: text.slice(cursor, index), match: false })
    parts.push({ text: match[0], match: true })
    cursor = index + match[0].length
  }
  if (cursor < text.length) parts.push({ text: text.slice(cursor), match: false })
  return parts
}
