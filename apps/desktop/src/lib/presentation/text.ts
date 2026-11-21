export function pluralize(count: number, singular: string, plural: string): string {
  return count === 1 ? singular : plural
}

export function capitalize(value: string): string {
  return value.length === 0 ? value : value[0]!.toUpperCase() + value.slice(1)
}
