import {
  acknowledgeMainWindowNavigationTarget,
  existingMainWindowSessionTargets,
  noteInteraction,
  onMainWindowNavigationTarget,
  peekMainWindowNavigationTarget,
  type MainWindowNavigationRequest,
  type MainWindowSectionId,
  type MainWindowSessionIdentity,
} from "../../lib/ipc"
import type { MainViewId } from "../../lib/navigation/mainViews"
import type { CHECK_LABELS } from "../../lib/presentation/checks"
import { localSessionKey } from "../../lib/presentation/localIdentity"
import { sessionFilterId, type SessionFilter } from "../../lib/sessionFilters"
import { sessionKey, type SessionSubject } from "../../lib/sessionSubject"

import type { MainActivitySession } from "./MainActivitySession"

const HISTORY_LIMIT = 100
const NATIVE_VIEW_IDS = {
  overview: "overview",
  activity: "activity",
  burnChecks: "burnChecks",
} as const satisfies Record<MainWindowSectionId, MainViewId>

export type MainDestination = {
  section: MainViewId
  filter?: SessionFilter
  subject?: SessionSubject | null
  check?: keyof typeof CHECK_LABELS
}

export type MainWindowNavigationSnapshot = {
  selected: MainViewId
  destination: MainDestination
  destinationRevision: number
  canBack: boolean
  canForward: boolean
  visited: readonly MainViewId[]
  /** Count accepted cross-window requests, including requests for the current section. */
  requests: number
}

function normalizeDestination(destination: MainDestination): MainDestination {
  if (destination.section === "activity") {
    return {
      section: "activity",
      filter: destination.filter ?? { kind: "all" },
      subject: destination.subject ?? null,
    }
  }
  if (destination.section === "burnChecks") {
    return destination.check
      ? { section: "burnChecks", check: destination.check }
      : { section: "burnChecks" }
  }
  return { section: destination.section }
}

function destinationKey(destination: MainDestination): string {
  return JSON.stringify([
    destination.section,
    destination.filter ? sessionFilterId(destination.filter) : null,
    destination.subject ? sessionKey(destination.subject) : null,
    destination.check ?? null,
  ])
}

/** Own cross-window requests and the retained main window's bounded history. */
export class MainWindowNavigationSession {
  private snapshot: MainWindowNavigationSnapshot = {
    selected: "overview",
    destination: { section: "overview" },
    destinationRevision: 0,
    canBack: false,
    canForward: false,
    visited: ["overview"],
    requests: 0,
  }
  private readonly activity: MainActivitySession | undefined
  private history: MainDestination[] = [{ section: "overview" }]
  private index = 0
  private restoring = false
  private targetRevision = 0
  private historyRevision = 0
  private inventoryRevision = 0
  private reconciliationDirty = false
  private reconciliationTask: Promise<void> | null = null
  private generation = 0
  private listeners = new Set<() => void>()
  private stop: (() => void) | null = null

  constructor(activity?: MainActivitySession) {
    this.activity = activity
    if (!activity) return
    activity.onNavigation = (origin) => {
      if (this.restoring) return
      if (origin === "automatic" && this.snapshot.selected !== "activity") return
      const { filter, subject } = activity.getSnapshot()
      this.commit({ section: "activity", filter, subject }, origin === "automatic", origin)
    }
    activity.onDeleted = (subject) => this.pruneSubject(subject)
    activity.onSessionInventoryInvalidated = () => this.requestDeletedSubjectReconciliation()
  }

  getSnapshot = (): MainWindowNavigationSnapshot => this.snapshot

  subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener)
    if (this.listeners.size === 1) void this.start()
    return () => {
      this.listeners.delete(listener)
      if (this.listeners.size === 0) this.dispose()
    }
  }

  select(section: MainViewId): void {
    if (section === "activity" && this.activity) {
      this.navigate(
        { section, filter: { kind: "all" }, subject: this.activity.getSnapshot().subject },
        false,
      )
      return
    }
    this.navigate({ section })
  }

  navigate(destination: MainDestination, revealDetail = true): void {
    this.commit(destination, false, "user")
    if (revealDetail) this.revealDetail()
  }

  back = (): void => {
    if (this.index === 0) return
    this.index -= 1
    this.restore("user")
    this.revealDetail()
    noteInteraction({ kind: "navigationHistoryMoved", direction: "back" })
  }

  forward = (): void => {
    if (this.index >= this.history.length - 1) return
    this.index += 1
    this.restore("user")
    this.revealDetail()
    noteInteraction({ kind: "navigationHistoryMoved", direction: "forward" })
  }

  private revealDetail(): void {
    if (this.snapshot.selected === "activity" && this.activity?.getSnapshot().subject)
      this.activity.revealDetail()
  }

  private commit(
    destination: MainDestination,
    replace: boolean,
    origin: "user" | "automatic",
  ): void {
    const normalized = normalizeDestination(destination)
    if (destinationKey(normalized) === destinationKey(this.history[this.index]!)) {
      if (!replace) this.publish()
      return
    }
    if (replace) {
      this.history[this.index] = normalized
    } else {
      this.history = [...this.history.slice(0, this.index + 1), normalized].slice(
        -HISTORY_LIMIT,
      )
      this.index = this.history.length - 1
    }
    this.noteHistoryChanged()
    this.restore(origin, !replace && origin === "user")
  }

  private restore(origin: "user" | "automatic", reportFilterSelection = false): void {
    const destination = this.history[this.index]!
    this.restoring = true
    try {
      if (destination.section === "activity" && this.activity) {
        this.activity.restoreNavigation(
          destination.filter ?? { kind: "all" },
          destination.subject ?? null,
          origin,
          reportFilterSelection,
        )
      }
    } finally {
      this.restoring = false
    }
    this.publish()
  }

  private pruneSubject(subject: SessionSubject): void {
    const key = sessionKey(subject)
    this.pruneDestinations(
      (destination) => !destination.subject || sessionKey(destination.subject) !== key,
    )
  }

  private rootIdentity(subject: SessionSubject): MainWindowSessionIdentity {
    return {
      agent: subject.agent,
      sessionId: subject.subagent?.parentSessionId ?? subject.sessionId,
      wslDistro: subject.wslDistro ?? null,
    }
  }

  private requestDeletedSubjectReconciliation(): void {
    this.inventoryRevision += 1
    this.reconciliationDirty = true
    this.startDeletedSubjectReconciliation()
  }

  private startDeletedSubjectReconciliation(): void {
    if (this.reconciliationTask) return
    const task = this.runDeletedSubjectReconciliation().finally(() => {
      if (this.reconciliationTask !== task) return
      this.reconciliationTask = null
      if (this.reconciliationDirty) this.startDeletedSubjectReconciliation()
    })
    this.reconciliationTask = task
  }

  private async runDeletedSubjectReconciliation(): Promise<void> {
    while (this.reconciliationDirty) {
      this.reconciliationDirty = false
      await this.reconcileDeletedSubjectsOnce()
    }
  }

  private async reconcileDeletedSubjectsOnce(): Promise<void> {
    const subjects = this.history.flatMap((destination) =>
      destination.subject ? [destination.subject] : [],
    )
    const current = this.activity?.getSnapshot().subject
    if (current) subjects.push(current)
    const targets = [
      ...new Map(
        subjects.map((subject) => {
          const target = this.rootIdentity(subject)
          return [localSessionKey(target.agent, target.sessionId, target.wslDistro), target]
        }),
      ).values(),
    ]
    if (targets.length === 0) return
    const inventoryRevision = this.inventoryRevision
    const historyRevision = this.historyRevision
    const existing = await existingMainWindowSessionTargets(targets).catch(() => null)
    if (!existing) return
    if (
      inventoryRevision !== this.inventoryRevision ||
      historyRevision !== this.historyRevision
    ) {
      this.reconciliationDirty = true
      return
    }
    const available = new Set(
      existing.map((target) =>
        localSessionKey(target.agent, target.sessionId, target.wslDistro),
      ),
    )
    const isAvailable = (subject: SessionSubject) => {
      const target = this.rootIdentity(subject)
      return available.has(localSessionKey(target.agent, target.sessionId, target.wslDistro))
    }
    const checked = new Set(
      targets.map((target) =>
        localSessionKey(target.agent, target.sessionId, target.wslDistro),
      ),
    )
    const currentAfterCheck = this.activity?.getSnapshot().subject
    const currentTarget = currentAfterCheck ? this.rootIdentity(currentAfterCheck) : null
    const deletedCurrent =
      currentAfterCheck &&
      currentTarget &&
      checked.has(
        localSessionKey(currentTarget.agent, currentTarget.sessionId, currentTarget.wslDistro),
      ) &&
      !isAvailable(currentAfterCheck)
        ? currentAfterCheck
        : null
    this.pruneDestinations(
      (destination) => !destination.subject || isAvailable(destination.subject),
      false,
    )
    if (deletedCurrent) this.activity?.removeDeletedSubject(deletedCurrent)
  }

  private pruneDestinations(
    keep: (destination: MainDestination) => boolean,
    scheduleReconciliation = true,
  ): void {
    const retained = this.history.filter(keep)
    if (retained.length === this.history.length) return
    const retainedThroughCurrent = this.history.slice(0, this.index + 1).filter(keep).length
    this.history = retained
    if (this.history.length === 0) {
      this.history = [
        {
          section: "activity",
          filter: this.activity?.getSnapshot().filter ?? { kind: "all" },
          subject: null,
        },
      ]
      this.index = 0
    } else {
      this.index = Math.min(Math.max(0, retainedThroughCurrent - 1), this.history.length - 1)
    }
    this.noteHistoryChanged(scheduleReconciliation)
    this.restore("automatic")
  }

  private noteHistoryChanged(scheduleReconciliation = true): void {
    this.historyRevision += 1
    if (scheduleReconciliation && this.reconciliationTask) this.reconciliationDirty = true
  }

  private publish(): void {
    const destination = this.history[this.index]!
    this.snapshot = {
      ...this.snapshot,
      selected: destination.section,
      destination,
      destinationRevision: this.snapshot.destinationRevision + 1,
      canBack: this.index > 0,
      canForward: this.index < this.history.length - 1,
      visited: this.snapshot.visited.includes(destination.section)
        ? this.snapshot.visited
        : [...this.snapshot.visited, destination.section],
    }
    for (const listener of this.listeners) listener()
  }

  private destinationForRequest(request: MainWindowNavigationRequest): MainDestination {
    if (request.destination.section !== "activity") {
      return { section: NATIVE_VIEW_IDS[request.destination.section] }
    }
    const activity = this.activity?.getSnapshot()
    return {
      section: "activity",
      filter: activity?.filter ?? { kind: "all" },
      subject: request.destination.target ?? activity?.subject ?? null,
    }
  }

  private applyAndAcknowledge(request: MainWindowNavigationRequest): void {
    if (request.revision < this.targetRevision) return
    if (request.revision > this.targetRevision) {
      this.targetRevision = request.revision
      this.snapshot = { ...this.snapshot, requests: this.snapshot.requests + 1 }
      this.navigate(this.destinationForRequest(request))
    }
    const generation = this.rendererGeneration()
    if (generation === null) return
    void acknowledgeMainWindowNavigationTarget(generation, request.revision).catch(() => {
      console.error("The main window could not acknowledge its navigation target.")
    })
  }

  private rendererGeneration(): number | null {
    const generation = window.__ANTIBURN_WINDOW_GENERATION__
    return typeof generation === "number" && Number.isSafeInteger(generation)
      ? generation
      : null
  }

  private async start(): Promise<void> {
    const generation = ++this.generation
    const stop = await onMainWindowNavigationTarget((request) => {
      if (generation !== this.generation) return
      this.applyAndAcknowledge(request)
      void this.peek(generation)
    }).catch(() => null)
    if (generation !== this.generation) {
      stop?.()
      return
    }
    this.stop = stop
    await this.peek(generation)
  }

  private async peek(generation: number): Promise<void> {
    const rendererGeneration = this.rendererGeneration()
    if (rendererGeneration === null) return
    const request = await peekMainWindowNavigationTarget(rendererGeneration).catch(() => null)
    if (generation === this.generation && request) this.applyAndAcknowledge(request)
  }

  private dispose(): void {
    this.generation += 1
    this.stop?.()
    this.stop = null
  }
}
