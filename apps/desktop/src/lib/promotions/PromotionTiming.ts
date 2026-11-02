export const PROMOTION_FTUE_KEY = "antiburn.promotions.ftue.v1"
export const PROMOTION_FTUE_DELAY_MS = 24 * 60 * 60 * 1000

interface OnboardingState {
  mode: "pending" | "firstRun" | "steady"
  flow: string
}

interface TimingStorage {
  read(): string | null
  write(value: string): void
  now(): number
}

/** Keep new-user promotions out of the onboarding session. */
export class PromotionTiming {
  private firstRunThisSession = false
  private history: { completedAt: number | null } | null = null

  private readonly storage: TimingStorage

  constructor(storage: TimingStorage) {
    this.storage = storage
    try {
      const stored: unknown = JSON.parse(storage.read() ?? "null")
      if (stored && typeof stored === "object" && "completedAt" in stored) {
        const completedAt = stored.completedAt
        if (
          completedAt === null ||
          (typeof completedAt === "number" && Number.isFinite(completedAt) && completedAt >= 0)
        ) {
          this.history = { completedAt }
        }
      }
    } catch {
      // Keep the session usable when storage is unavailable.
    }
  }

  private save(completedAt: number | null): void {
    this.history = { completedAt }
    try {
      this.storage.write(JSON.stringify(this.history))
    } catch {
      // Keep the delay in memory when storage is unavailable.
    }
  }

  ready(progress: OnboardingState): boolean {
    if (progress.mode === "pending") return false
    if (progress.mode === "firstRun") {
      this.firstRunThisSession = true
      if (progress.flow !== "done") {
        if (!this.history || this.history.completedAt !== null) this.save(null)
      } else if (!this.history || this.history.completedAt === null) {
        this.save(this.storage.now())
      }
      return false
    }
    if (this.history?.completedAt === null) {
      this.firstRunThisSession = true
      this.save(this.storage.now())
    }
    const completedAt = this.history?.completedAt
    return (
      !this.firstRunThisSession &&
      (completedAt == null || this.storage.now() - completedAt >= PROMOTION_FTUE_DELAY_MS)
    )
  }
}
