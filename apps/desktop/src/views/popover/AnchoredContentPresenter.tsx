import { Component, type ReactNode, type SyntheticEvent, type TransitionEvent } from "react"

export interface AnchoredPresentation<T> {
  generation: number
  value: T
}

interface AnchoredContentPresenterProps<T> {
  requestedGeneration: number
  presented: AnchoredPresentation<T> | null
  candidate: AnchoredPresentation<T> | null
  coldLoading: boolean
  busy: boolean
  loading: ReactNode
  renderContent: (value: T) => ReactNode
  retargetCommitRequired: boolean
  initialPresentationCommitRequired: boolean
  commitRetarget: (generation: number, height: number | null) => Promise<boolean>
  acknowledge: (generation: number, height: number) => Promise<boolean>
  onPromote: (generation: number) => void
  onDiscard: (generation: number) => void
}

type Slot<T> = { kind: "loading" } | { kind: "content"; presentation: AnchoredPresentation<T> }

type PresenterPhase = "idle" | "measuring" | "awaiting-frame" | "crossfading" | "reversing"

interface PresenterState<T> {
  slots: [Slot<T> | null, Slot<T> | null]
  stableIndex: 0 | 1
  phase: PresenterPhase
}

interface Acknowledgement {
  generation: number
  height: number
  slotIndex: 0 | 1
}

function measuredHeight(node: HTMLElement): number {
  return Math.ceil(node.getBoundingClientRect().height)
}

function milliseconds(value: string): number {
  const trimmed = value.trim()
  if (trimmed.endsWith("ms")) return Number.parseFloat(trimmed)
  if (trimmed.endsWith("s")) return Number.parseFloat(trimmed) * 1000
  return 0
}

function transitionMilliseconds(node: HTMLElement): number {
  const style = getComputedStyle(node)
  const durations = style.transitionDuration.split(",").map(milliseconds)
  const delays = style.transitionDelay.split(",").map(milliseconds)
  return durations.reduce(
    (maximum, duration, index) =>
      Math.max(maximum, duration + (delays[index % Math.max(delays.length, 1)] ?? 0)),
    0,
  )
}

function contentSlot<T>(presentation: AnchoredPresentation<T>): Slot<T> {
  return { kind: "content", presentation }
}

/** This blocks host actions while current content remains available to assistive technology. */
function preventHostInteraction(event: SyntheticEvent): void {
  event.preventDefault()
  event.stopPropagation()
}

/** Keeps stable anchored content visible until native geometry accepts its replacement. */
export class AnchoredContentPresenter<T> extends Component<
  AnchoredContentPresenterProps<T>,
  PresenterState<T>
> {
  state: PresenterState<T>

  private mounted = false
  private readonly slotNodes: [HTMLDivElement | null, HTMLDivElement | null] = [null, null]
  private readonly observers: [ResizeObserver | null, ResizeObserver | null] = [null, null]
  private acknowledgement: Acknowledgement | null = null
  private retargetFrame: number | null = null
  private retargetPaintFrame: number | null = null
  private retargetCommitGeneration: number | null = null
  private retargetRevision = 0
  private retargetBarrierSettled: boolean
  private startFrame: number | null = null
  private resizeFrame: number | null = null
  private pendingStableHeight: number | null = null
  private lastStableMeasurement: { generation: number; height: number } | null = null
  private watchdog: ReturnType<typeof setTimeout> | null = null

  private readonly slotRefs = [
    (node: HTMLDivElement | null) => this.setSlotNode(0, node),
    (node: HTMLDivElement | null) => this.setSlotNode(1, node),
  ] as const

  constructor(props: AnchoredContentPresenterProps<T>) {
    super(props)
    const stable = props.presented
      ? contentSlot(props.presented)
      : props.coldLoading
        ? ({ kind: "loading" } as const)
        : null
    this.state = { slots: [stable, null], stableIndex: 0, phase: "idle" }
    this.retargetBarrierSettled = !props.retargetCommitRequired
  }

  componentDidMount(): void {
    this.mounted = true
    this.scheduleRetargetCommit()
    this.reconcile()
  }

  componentDidUpdate(): void {
    this.scheduleRetargetCommit()
    this.reconcile()
  }

  componentWillUnmount(): void {
    this.mounted = false
    this.acknowledgement = null
    this.observers.forEach((observer) => observer?.disconnect())
    this.observers[0] = null
    this.observers[1] = null
    this.clearWatchdog()
    this.retargetRevision += 1
    if (this.retargetFrame != null) cancelAnimationFrame(this.retargetFrame)
    if (this.retargetPaintFrame != null) cancelAnimationFrame(this.retargetPaintFrame)
    if (this.startFrame != null) cancelAnimationFrame(this.startFrame)
    if (this.resizeFrame != null) cancelAnimationFrame(this.resizeFrame)
    this.retargetFrame = null
    this.retargetPaintFrame = null
    this.retargetCommitGeneration = null
    this.retargetBarrierSettled = !this.props.retargetCommitRequired
    this.startFrame = null
    this.resizeFrame = null
  }

  private scheduleRetargetCommit(): void {
    if (!this.props.retargetCommitRequired) {
      if (this.retargetFrame != null) cancelAnimationFrame(this.retargetFrame)
      this.retargetFrame = null
      if (this.retargetPaintFrame != null) cancelAnimationFrame(this.retargetPaintFrame)
      this.retargetPaintFrame = null
      this.retargetCommitGeneration = null
      this.retargetBarrierSettled = true
      return
    }
    const generation = this.props.requestedGeneration
    if (this.retargetCommitGeneration === generation) return
    if (this.retargetFrame != null) cancelAnimationFrame(this.retargetFrame)
    if (this.retargetPaintFrame != null) cancelAnimationFrame(this.retargetPaintFrame)
    const revision = ++this.retargetRevision
    this.retargetCommitGeneration = generation
    this.retargetBarrierSettled = false
    this.retargetFrame = requestAnimationFrame(() => {
      this.retargetFrame = null
      if (
        !this.mounted ||
        !this.props.retargetCommitRequired ||
        this.props.requestedGeneration !== generation
      ) {
        return
      }
      this.retargetPaintFrame = requestAnimationFrame(() => {
        this.retargetPaintFrame = null
        if (
          !this.mounted ||
          this.retargetRevision !== revision ||
          !this.props.retargetCommitRequired ||
          this.props.requestedGeneration !== generation
        ) {
          return
        }
        const stable = this.state.slots[this.state.stableIndex]
        const stableNode = this.slotNodes[this.state.stableIndex]
        const height =
          stable?.kind === "content" && stableNode ? measuredHeight(stableNode) : null
        void this.props
          .commitRetarget(generation, height && height > 0 ? height : null)
          .then((committed) => {
            if (committed) this.finishRetargetCommit(generation, revision)
          })
          .catch(() => undefined)
      })
    })
  }

  private finishRetargetCommit(generation: number, revision: number): void {
    if (
      !this.mounted ||
      this.retargetRevision !== revision ||
      this.props.requestedGeneration !== generation
    ) {
      return
    }
    this.retargetBarrierSettled = true
    this.reconcile()
  }

  private reconcile(): void {
    if (!this.mounted) return
    const incomingIndex = this.incomingIndex()
    const incoming = this.state.slots[incomingIndex]

    if (incoming?.kind === "content") {
      const stale = incoming.presentation.generation !== this.props.requestedGeneration
      if (stale) {
        if (this.state.phase === "crossfading") this.reverseCrossfade()
        else if (this.state.phase !== "reversing") this.discardHiddenIncoming()
      }
      return
    }

    const stableIndex = this.state.stableIndex
    const stable = this.state.slots[stableIndex]
    if ((!stable || stable.kind === "loading") && this.props.presented) {
      const slots = [...this.state.slots] as PresenterState<T>["slots"]
      slots[stableIndex] = contentSlot(this.props.presented)
      this.setState({ slots }, () => this.measureSlot(stableIndex))
      return
    }

    if (this.props.retargetCommitRequired && !this.retargetBarrierSettled) return

    if (this.state.phase !== "idle") return
    const candidate = this.props.candidate
    if (candidate?.generation === this.props.requestedGeneration) {
      const stable = this.state.slots[this.state.stableIndex]
      if (
        stable?.kind !== "content" ||
        stable.presentation.generation !== candidate.generation
      ) {
        this.stage(candidate)
      }
      return
    }
  }

  private incomingIndex(): 0 | 1 {
    return this.state.stableIndex === 0 ? 1 : 0
  }

  private stage(candidate: AnchoredPresentation<T>): void {
    const incomingIndex = this.incomingIndex()
    const slots = [...this.state.slots] as PresenterState<T>["slots"]
    slots[incomingIndex] = contentSlot(candidate)
    this.acknowledgement = null
    this.setState({ slots, phase: "measuring" }, () => this.measureSlot(incomingIndex))
  }

  private setSlotNode(index: 0 | 1, node: HTMLDivElement | null): void {
    this.observers[index]?.disconnect()
    this.observers[index] = null
    this.slotNodes[index] = node
    if (!node) return
    if (typeof ResizeObserver !== "undefined") {
      const observer = new ResizeObserver(() => this.measureSlot(index))
      observer.observe(node)
      this.observers[index] = observer
    }
    this.measureSlot(index)
  }

  private measureSlot(index: 0 | 1): void {
    const node = this.slotNodes[index]
    const slot = this.state.slots[index]
    if (!node || !slot || slot.kind !== "content") return
    const height = measuredHeight(node)
    if (height <= 0) return

    if (index === this.incomingIndex() && this.state.phase === "measuring") {
      this.acknowledgeCandidate(index, slot.presentation.generation, height)
      return
    }
    if (index === this.state.stableIndex && this.state.phase === "idle") {
      this.reportStableHeight(slot.presentation.generation, height)
    }
  }

  private acknowledgeCandidate(index: 0 | 1, generation: number, height: number): void {
    if (this.acknowledgement) return
    const acknowledgement = { generation, height, slotIndex: index }
    this.acknowledgement = acknowledgement
    this.setState({ phase: "awaiting-frame" })
    void this.props
      .acknowledge(generation, height)
      .then(() => this.handleAcknowledgement(acknowledgement))
      .catch(() => this.handleAcknowledgementRejection(acknowledgement))
  }

  private handleAcknowledgement(acknowledgement: Acknowledgement): void {
    if (!this.mounted || this.acknowledgement !== acknowledgement) return
    const slot = this.state.slots[acknowledgement.slotIndex]
    const current =
      slot?.kind === "content" &&
      slot.presentation.generation === acknowledgement.generation &&
      acknowledgement.generation === this.props.requestedGeneration
    if (!current) {
      this.discardHiddenIncoming()
      return
    }
    this.startFrame = requestAnimationFrame(() => {
      this.startFrame = null
      if (!this.mounted || this.acknowledgement !== acknowledgement) return
      if (acknowledgement.generation !== this.props.requestedGeneration) {
        this.discardHiddenIncoming()
        return
      }
      this.setState({ phase: "crossfading" }, () => this.startWatchdog("promote"))
    })
  }

  private handleAcknowledgementRejection(acknowledgement: Acknowledgement): void {
    if (!this.mounted || this.acknowledgement !== acknowledgement) return
    this.discardHiddenIncoming()
  }

  private reverseCrossfade(): void {
    this.clearWatchdog()
    this.setState({ phase: "reversing" }, () => this.startWatchdog("discard"))
  }

  private discardHiddenIncoming(): void {
    const incomingIndex = this.incomingIndex()
    const incoming = this.state.slots[incomingIndex]
    const generation = incoming?.kind === "content" ? incoming.presentation.generation : null
    if (this.startFrame != null) cancelAnimationFrame(this.startFrame)
    this.startFrame = null
    this.acknowledgement = null
    this.clearWatchdog()
    const slots = [...this.state.slots] as PresenterState<T>["slots"]
    slots[incomingIndex] = null
    this.setState({ slots, phase: "idle" }, () => {
      if (generation != null) this.props.onDiscard(generation)
      this.reconcile()
    })
  }

  private promoteIncoming(): void {
    const incomingIndex = this.incomingIndex()
    const incoming = this.state.slots[incomingIndex]
    if (incoming?.kind !== "content") return
    const generation = incoming.presentation.generation
    const height =
      this.acknowledgement?.height ?? measuredHeight(this.slotNodes[incomingIndex]!)
    const slots = [...this.state.slots] as PresenterState<T>["slots"]
    slots[this.state.stableIndex] = null
    this.lastStableMeasurement = { generation, height }
    this.acknowledgement = null
    this.clearWatchdog()
    this.setState({ slots, stableIndex: incomingIndex, phase: "idle" }, () => {
      this.props.onPromote(generation)
      this.measureSlot(incomingIndex)
      this.reconcile()
    })
  }

  private reportStableHeight(generation: number, height: number): void {
    if (this.lastStableMeasurement?.generation !== generation) {
      this.lastStableMeasurement = { generation, height }
      if (this.props.initialPresentationCommitRequired) {
        void this.props.acknowledge(generation, height).catch(() => undefined)
      }
      return
    }
    if (this.lastStableMeasurement.height === height) return
    this.pendingStableHeight = height
    if (this.resizeFrame != null) return
    this.resizeFrame = requestAnimationFrame(() => {
      this.resizeFrame = null
      const nextHeight = this.pendingStableHeight
      this.pendingStableHeight = null
      const stable = this.state.slots[this.state.stableIndex]
      if (
        !this.mounted ||
        nextHeight == null ||
        this.state.phase !== "idle" ||
        stable?.kind !== "content" ||
        stable.presentation.generation !== generation ||
        this.lastStableMeasurement?.height === nextHeight
      ) {
        return
      }
      this.lastStableMeasurement = { generation, height: nextHeight }
      void this.props.acknowledge(generation, nextHeight).catch(() => undefined)
    })
  }

  private startWatchdog(action: "promote" | "discard"): void {
    this.clearWatchdog()
    const node = this.slotNodes[this.incomingIndex()]
    if (!node) return
    const transition = transitionMilliseconds(node)
    this.watchdog = setTimeout(
      () => {
        this.watchdog = null
        if (action === "promote") this.promoteIncoming()
        else this.discardHiddenIncoming()
      },
      transition + transition / 2,
    )
  }

  private clearWatchdog(): void {
    if (this.watchdog != null) clearTimeout(this.watchdog)
    this.watchdog = null
  }

  private handleTransitionEnd = (
    index: 0 | 1,
    event: TransitionEvent<HTMLDivElement>,
  ): void => {
    if (event.target !== event.currentTarget || event.propertyName !== "opacity") return
    if (index !== this.incomingIndex()) return
    if (this.state.phase === "crossfading") this.promoteIncoming()
    else if (this.state.phase === "reversing") this.discardHiddenIncoming()
  }

  private slotState(
    index: 0 | 1,
  ): "stable" | "staged" | "incoming" | "outgoing" | "discarding" {
    if (index === this.state.stableIndex) {
      return this.state.phase === "crossfading" || this.state.phase === "reversing"
        ? "outgoing"
        : "stable"
    }
    if (this.state.phase === "crossfading") return "incoming"
    if (this.state.phase === "reversing") return "discarding"
    return "staged"
  }

  private slotOpacity(index: 0 | 1): string {
    if (index === this.state.stableIndex) {
      return this.state.phase === "crossfading" ? "opacity-0" : "opacity-100"
    }
    return this.state.phase === "crossfading" ? "opacity-100" : "opacity-0"
  }

  render(): ReactNode {
    return (
      <div
        className="relative h-full overflow-hidden"
        data-testid="anchored-content-presenter"
        data-presenter-phase={this.state.phase}
        aria-busy={this.props.busy || undefined}
      >
        {this.state.slots.map((slot, indexValue) => {
          if (!slot) return null
          const index = indexValue as 0 | 1
          const stable = index === this.state.stableIndex
          const generation = slot.kind === "content" ? slot.presentation.generation : undefined
          return (
            <div
              key={index}
              inert={stable ? undefined : true}
              aria-hidden={stable ? undefined : true}
              data-generation={generation}
              data-slot-state={this.slotState(index)}
              onClickCapture={preventHostInteraction}
              onKeyDownCapture={preventHostInteraction}
              onTransitionEnd={(event) => this.handleTransitionEnd(index, event)}
              className={`ui-anchored-content-transition pointer-events-none h-full overflow-y-auto ${
                stable ? "relative" : "absolute inset-0"
              } ${this.slotOpacity(index)}`}
            >
              <div ref={this.slotRefs[index]}>
                {slot.kind === "loading"
                  ? this.props.loading
                  : this.props.renderContent(slot.presentation.value)}
              </div>
            </div>
          )
        })}
      </div>
    )
  }
}
