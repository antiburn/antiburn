/** Parse a `YYYY-MM-DD` reader-local date as a local `Date` at midnight. */
function localDateOf(localDate: string): Date {
  const [year, month, day] = localDate.split("-").map(Number)
  return new Date(year ?? 1970, (month ?? 1) - 1, day ?? 1)
}

/** "Mon 14 Sep" for a reader-local date. */
export function dayLabel(localDate: string): string {
  const date = localDateOf(localDate)
  const weekday = date.toLocaleDateString("en-US", { weekday: "short" })
  const month = date.toLocaleDateString("en-US", { month: "short" })
  return `${weekday} ${date.getDate()} ${month}`
}

/** "14 Sep" for an axis label. */
export function axisDayLabel(localDate: string): string {
  const date = localDateOf(localDate)
  return `${date.getDate()} ${date.toLocaleDateString("en-US", { month: "short" })}`
}
