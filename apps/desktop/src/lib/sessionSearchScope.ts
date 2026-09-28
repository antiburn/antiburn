export interface SessionSearchScope {
  days: number
  fromEpoch: number
  throughEpoch: number
  timeZone: string
}

export function activitySearchScope(days: number, now = new Date()): SessionSearchScope {
  const start = new Date(now)
  start.setHours(0, 0, 0, 0)
  start.setDate(start.getDate() - (days - 1))
  return {
    days,
    fromEpoch: Math.floor(start.getTime() / 1000),
    throughEpoch: Math.floor(now.getTime() / 1000),
    timeZone: Intl.DateTimeFormat().resolvedOptions().timeZone,
  }
}

export function searchScopeLabel(scope: SessionSearchScope | null): string {
  return scope
    ? `Last ${scope.days} ${scope.days === 1 ? "day" : "days"}`
    : "All retained content"
}
