import { noteInteraction } from "../ipc"
import type { PromotionCampaign, PromotionDefinition } from "../../promotions/types"

export type PromotionInteractionAction =
  | "announcement_viewed"
  | "setup_viewed"
  | "primary_activated"
  | "learn_more_activated"
  | "command_copied"
  | "dismissed"

export interface PromotionAnalyticsContext {
  campaign: PromotionCampaign | null
  origin: "user" | "automatic"
  readonly reported: Set<PromotionInteractionAction>
}

export const PROMOTION_SEEN_KEY = "antiburn.promotions.seen.v1"
export const PROMOTION_LAUNCHER_OPENED_KEY = "antiburn.promotions.launcher-opened.v1"

export interface PromotionEnvironment {
  isManualReady(): boolean
  loadReady(): Promise<boolean>
  onReady(listener: (ready: boolean) => void): Promise<() => void>
  loadVisible(): Promise<boolean>
  onVisible(listener: (visible: boolean) => void): Promise<() => void>
  isForeground(): boolean
  onForeground(listener: () => void): () => void
  readSeen(): string | null
  writeSeen(value: string): void
  readLauncherOpened(): string | null
  writeLauncherOpened(value: string): void
}

export interface PromotionSnapshot {
  selected: PromotionDefinition | null
  screen: "announcement" | "setup" | "catalog"
  visible: boolean
  manual: boolean
  focusPanel: boolean
  launcherOpened: readonly string[]
}

/** Own the panel lifecycle outside React. */
export class PromotionSession {
  private snapshot: PromotionSnapshot = {
    selected: null,
    screen: "announcement",
    visible: false,
    manual: false,
    focusPanel: false,
    launcherOpened: [],
  }
  private listeners = new Set<() => void>()
  private stops: (() => void)[] = []
  private generation = 0
  private ready = false
  private visible = false
  private automaticSelected = false
  private revealPending = false
  private seen = new Set<string>()
  private openingContext: PromotionAnalyticsContext = {
    campaign: null,
    origin: "automatic",
    reported: new Set(),
  }

  readonly catalog: readonly PromotionDefinition[]
  private readonly environment: PromotionEnvironment

  constructor(catalog: readonly PromotionDefinition[], environment: PromotionEnvironment) {
    this.catalog = catalog
    this.environment = environment
  }

  getSnapshot = (): PromotionSnapshot => this.snapshot

  subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener)
    if (this.listeners.size === 1) void this.start()
    return () => {
      this.listeners.delete(listener)
      if (this.listeners.size === 0) {
        this.generation += 1
        for (const stop of this.stops.splice(0)) stop()
      }
    }
  }

  private publish(patch: Partial<PromotionSnapshot>): void {
    this.snapshot = { ...this.snapshot, ...patch }
    for (const listener of this.listeners) listener()
  }

  private async start(): Promise<void> {
    const generation = ++this.generation
    try {
      const stored: unknown = JSON.parse(this.environment.readLauncherOpened() ?? "[]")
      if (Array.isArray(stored)) {
        this.publish({
          launcherOpened: this.catalog
            .map((promo) => this.key(promo))
            .filter(
              (key) => stored.includes(key) || this.snapshot.launcherOpened.includes(key),
            ),
        })
      }
    } catch {
      // Keep the in-memory history when storage is unavailable.
    }
    try {
      const stored: unknown = JSON.parse(this.environment.readSeen() ?? "[]")
      if (Array.isArray(stored)) {
        for (const promo of this.catalog) {
          if (stored.includes(this.key(promo))) this.seen.add(this.key(promo))
        }
      }
    } catch {
      // Keep the in-memory history when storage is unavailable.
    }
    let readyRevision = 0
    let visibleRevision = 0
    const keep = async (pending: Promise<() => void>) => {
      const stop = await pending.catch(() => null)
      if (!stop) return
      if (generation !== this.generation) stop()
      else this.stops.push(stop)
    }
    this.stops.push(this.environment.onForeground(() => this.sync()))
    await Promise.all([
      keep(
        this.environment.onReady((ready) => {
          if (generation !== this.generation) return
          readyRevision += 1
          this.ready = ready
          this.sync()
        }),
      ),
      keep(
        this.environment.onVisible((visible) => {
          if (generation !== this.generation) return
          visibleRevision += 1
          this.visible = visible
          this.sync()
        }),
      ),
    ])
    const readyVersion = readyRevision
    const visibleVersion = visibleRevision
    const [ready, visible] = await Promise.all([
      this.environment.loadReady().catch(() => false),
      this.environment.loadVisible().catch(() => false),
    ])
    if (generation !== this.generation) return
    if (readyVersion === readyRevision) this.ready = ready
    if (visibleVersion === visibleRevision) this.visible = visible
    this.sync()
  }

  private sync(): void {
    const visible =
      this.visible && (this.snapshot.manual ? this.environment.isManualReady() : this.ready)
    if (
      !this.automaticSelected &&
      this.ready &&
      visible &&
      this.environment.isForeground() &&
      !this.snapshot.selected
    ) {
      const selected = this.catalog.find(
        (promo) => !promo.development_only && !this.seen.has(this.key(promo)),
      )
      if (selected) {
        this.automaticSelected = true
        this.beginOpening(selected, false)
        this.publish({ selected, screen: "announcement", manual: false, visible })
        return
      }
    }
    if (visible !== this.snapshot.visible) this.publish({ visible })
  }

  private key(promo: PromotionDefinition): string {
    return `${promo.id}@${promo.revision}`
  }

  presented = (): void => {
    const promo = this.snapshot.selected
    if (!promo || !this.snapshot.visible) return
    if (this.snapshot.screen !== "catalog") {
      this.record(this.snapshot.screen === "setup" ? "setup_viewed" : "announcement_viewed")
    }
    if (this.seen.has(this.key(promo))) return
    this.seen.add(this.key(promo))
    try {
      this.environment.writeSeen(JSON.stringify([...this.seen]))
    } catch {
      // Keep this session quiet when storage is unavailable.
    }
  }

  private beginOpening(promo: PromotionDefinition, manual: boolean): void {
    this.revealPending = true
    this.openingContext = {
      campaign: promo.development_only ? null : (promo.analytics?.campaign ?? null),
      origin: manual ? "user" : "automatic",
      reported: new Set(),
    }
  }

  takeReveal = (): boolean => {
    const pending = this.revealPending
    this.revealPending = false
    return pending
  }

  analyticsContext = (): PromotionAnalyticsContext => this.openingContext

  record = (
    action: PromotionInteractionAction,
    context: PromotionAnalyticsContext = this.analyticsContext(),
  ): void => {
    if (context.campaign && !context.reported.has(action)) {
      context.reported.add(action)
      noteInteraction({
        kind: "promotionInteraction",
        campaign: context.campaign,
        action,
        origin: context.origin,
      })
    }
  }

  open = (): void => {
    if (!this.catalog.length) return
    this.automaticSelected = true
    const selected = this.catalog[0]!
    this.beginOpening(selected, true)
    this.publish({
      selected,
      screen: this.catalog.length > 1 ? "catalog" : "announcement",
      manual: true,
      focusPanel: true,
    })
  }

  openFromLauncher = (id: string, animate = true): boolean => {
    const selected = this.catalog.find((promo) => promo.id === id && !promo.development_only)
    if (!selected || !this.visible || !this.environment.isManualReady()) return false
    const key = this.key(selected)
    const launcherOpened = this.snapshot.launcherOpened.includes(key)
      ? this.snapshot.launcherOpened
      : [...this.snapshot.launcherOpened, key]
    this.automaticSelected = true
    this.beginOpening(selected, true)
    this.revealPending = animate
    this.publish({
      selected,
      screen: "announcement",
      manual: true,
      focusPanel: true,
      visible: true,
      launcherOpened,
    })
    try {
      this.environment.writeLauncherOpened(JSON.stringify(launcherOpened))
    } catch {
      // Keep the badge state for this session when storage is unavailable.
    }
    return true
  }

  select = (id: string): void => {
    const selected = this.catalog.find((promo) => promo.id === id)
    if (selected) {
      this.beginOpening(selected, true)
      this.publish({ selected, screen: "announcement", manual: true, focusPanel: true })
    }
  }

  navigate = (screen: "announcement" | "setup"): void => {
    if (screen === "setup" && !this.snapshot.selected?.screens.setup) return
    this.publish({ screen, focusPanel: true })
  }

  close = (): void => {
    if (this.snapshot.selected && this.snapshot.screen !== "catalog") this.record("dismissed")
    this.automaticSelected = true
    this.publish({ selected: null, screen: "announcement", manual: false, focusPanel: false })
  }
}
