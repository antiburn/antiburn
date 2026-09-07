import { Component, useState, useSyncExternalStore } from "react"

import { ProviderGlyph } from "../components/providerUsage/ProviderUsagePrimitives"
import { Skeleton } from "../components/ui/Skeleton"
import {
  popoverPeekConcealed,
  popoverPeekPresented,
  popoverPeekRetargetReady,
  type PopoverPeekTarget,
} from "../lib/popoverPeekIpc"
import {
  PopoverPeekController,
  type PopoverPeekSnapshot,
} from "./popover/PopoverPeekController"
import {
  AnchoredContentPresenter,
  type AnchoredPresentation,
} from "./popover/AnchoredContentPresenter"
import { UsageView } from "./popover/UsageView"
import { ChecksPeek } from "./popover/ChecksView"

function reportConcealed(generation: number) {
  return (node: HTMLSpanElement | null) => {
    if (node) void popoverPeekConcealed(generation).catch(() => undefined)
  }
}

function ProviderPeekSkeleton({ testId = "popover-peek-loading" }: { testId?: string }) {
  return (
    <div className="min-h-[320px] px-4 py-3" data-testid={testId} data-loading-state="detailed">
      <div className="flex flex-col gap-3 rounded-[var(--radius-popover)] bg-surface-card p-3">
        <div className="flex items-center gap-2">
          <Skeleton className="h-[18px] w-[18px] shrink-0 rounded-full" />
          <div className="flex flex-col gap-1">
            <Skeleton className="h-3 w-24" />
            <Skeleton className="h-3 w-16" />
          </div>
        </div>
        <div className="flex flex-col gap-3">
          <div className="flex flex-col gap-1.5">
            <Skeleton className="h-3 w-20" />
            <Skeleton className="h-2 w-full rounded-full" />
          </div>
          <div className="flex flex-col gap-1.5">
            <Skeleton className="h-3 w-28" />
            <Skeleton className="h-2 w-full rounded-full" />
          </div>
          <div className="grid grid-cols-2 gap-2 border-t border-separator pt-2">
            <Skeleton className="h-8 w-full" />
            <Skeleton className="h-8 w-full" />
          </div>
        </div>
      </div>
    </div>
  )
}

const PROVIDER_LOADING_LABELS: Record<string, string> = {
  anthropic: "Claude",
  cursor: "Cursor",
  deepseek: "DeepSeek",
  github: "GitHub Copilot",
  google: "Gemini",
  mistral: "Mistral",
  openai: "Codex",
  openrouter: "OpenRouter",
  windsurf: "Windsurf",
  xai: "xAI",
}

const LOADING_DETAIL_DELAY_MS = 150

function providerLoadingLabel(provider: string): string {
  return PROVIDER_LOADING_LABELS[provider] ?? provider
}

class PeekLoading extends Component<{ target: PopoverPeekTarget }, { showDetails: boolean }> {
  state = { showDetails: false }
  private detailTimer: ReturnType<typeof setTimeout> | null = null

  componentDidMount(): void {
    this.detailTimer = setTimeout(
      () => this.setState({ showDetails: true }),
      LOADING_DETAIL_DELAY_MS,
    )
  }

  componentWillUnmount(): void {
    if (this.detailTimer != null) clearTimeout(this.detailTimer)
    this.detailTimer = null
  }

  render() {
    if (this.props.target.kind === "checks") {
      return <ProviderPeekSkeleton />
    }
    const { provider } = this.props.target
    const displayName = providerLoadingLabel(provider)
    return (
      <div>
        <span role="status" className="sr-only">
          Loading preview
        </span>
        {this.state.showDetails ? (
          <ProviderPeekSkeleton />
        ) : (
          <div
            className="min-h-[320px] px-4 py-3"
            data-testid="popover-peek-loading"
            data-loading-state="quiet"
          >
            <div className="flex items-center gap-2 rounded-[var(--radius-popover)] bg-surface-card px-3 py-2.5">
              <ProviderGlyph displayName={displayName} provider={provider} size={18} />
              <span className="truncate type-footnote font-medium text-label">
                {displayName}
              </span>
            </div>
          </div>
        )}
      </div>
    )
  }
}

function PeekStandbySkeleton() {
  return (
    <div className="min-h-[320px]" data-testid="popover-peek-standby">
      <div className="flex items-center gap-3 border-b border-separator px-4 py-3">
        <Skeleton className="h-5 w-5 shrink-0 rounded-full" />
        <div className="flex min-w-0 flex-1 flex-col gap-1">
          <Skeleton className="h-3.5 w-32" />
          <Skeleton className="h-3 w-20" />
        </div>
      </div>
      <div className="flex flex-col gap-3 p-3">
        <Skeleton className="h-20 w-full" />
        <Skeleton className="h-20 w-full" />
      </div>
    </div>
  )
}

function PeekUnavailable() {
  return (
    <p role="status" className="min-h-[320px] px-4 py-3 type-callout text-label-secondary">
      Preview unavailable. Move the pointer away, then try again.
    </p>
  )
}

function Standby({ generation }: { generation: number }) {
  return (
    <div className="h-full" aria-hidden>
      <PeekStandbySkeleton />
      <span ref={reportConcealed(generation)} hidden />
    </div>
  )
}

type PeekPayload =
  | { kind: "data"; data: NonNullable<PopoverPeekSnapshot["candidate"]>["data"] }
  | { kind: "unavailable" }

function presentedContent(
  snapshot: PopoverPeekSnapshot,
): AnchoredPresentation<PeekPayload> | null {
  const presented = snapshot.presented
  if (!presented) return null
  return {
    generation: presented.request.generation,
    value:
      presented.unavailable || !presented.data
        ? { kind: "unavailable" }
        : { kind: "data", data: presented.data },
  }
}

function candidateContent(
  snapshot: PopoverPeekSnapshot,
): AnchoredPresentation<PeekPayload> | null {
  if (snapshot.candidate) {
    return {
      generation: snapshot.candidate.request.generation,
      value: { kind: "data", data: snapshot.candidate.data },
    }
  }
  if (snapshot.failed) {
    return { generation: snapshot.failed.generation, value: { kind: "unavailable" } }
  }
  return null
}

function PeekPayloadContent({ payload }: { payload: PeekPayload }) {
  if (payload.kind === "unavailable") return <PeekUnavailable />
  if (payload.data.kind === "checks") {
    return <ChecksPeek presentation={payload.data.presentation} />
  }
  return (
    <>
      <div data-popover-peek-usage className="pt-2">
        <UsageView
          summary={payload.data.summary}
          live={payload.data.live}
          onBack={() => undefined}
          embedded
        />
      </div>
    </>
  )
}

function PeekContent({
  snapshot,
  controller,
}: {
  snapshot: PopoverPeekSnapshot
  controller: PopoverPeekController
}) {
  const request = snapshot.requested
  if (!request.target) return <Standby generation={request.generation} />
  const presented = presentedContent(snapshot)
  return (
    <AnchoredContentPresenter
      key={request.initialPresentation ? request.generation : "retained"}
      requestedGeneration={request.generation}
      presented={presented}
      candidate={candidateContent(snapshot)}
      coldLoading={snapshot.coldLoading}
      busy={snapshot.presented?.request.generation !== request.generation}
      loading={<PeekLoading target={request.target} />}
      renderContent={(payload) => <PeekPayloadContent payload={payload} />}
      retargetCommitRequired={request.retargetCommitRequired}
      initialPresentationCommitRequired={
        request.initialPresentation != null && !request.retargetCommitRequired
      }
      commitRetarget={popoverPeekRetargetReady}
      acknowledge={popoverPeekPresented}
      onPromote={controller.promote}
      onDiscard={controller.discard}
    />
  )
}

/** Renders fresh, generation-checked data in the passive companion window. */
export function PopoverPeekView() {
  const [controller] = useState(() => new PopoverPeekController())
  const snapshot = useSyncExternalStore(
    controller.subscribe,
    controller.getSnapshot,
    controller.getSnapshot,
  )
  const commitRenderer = (node: HTMLDivElement | null) => controller.commitRenderer(node)

  return (
    <div ref={commitRenderer} className="h-full text-label">
      <PeekContent snapshot={snapshot} controller={controller} />
    </div>
  )
}
