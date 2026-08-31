import { invoke } from "@tauri-apps/api/core"
import { listen, type UnlistenFn } from "@tauri-apps/api/event"

import type { AnchorRegion } from "./anchorRegion"
import type {
  AnchoredWindowLifecycleEvent,
  AnchoredWindowRenderRequest,
  AnchoredWindowRequest,
  AnchoredWindowState,
} from "./anchoredTrigger"
import { hasShell, type LiveUsageSummaryPayload, type ProviderUsageSummaryPayload } from "./ipc"

export const POPOVER_PEEK_LABEL = "popover-peek"

/** A passive preview target owned by the popover. */
export type PopoverPeekTarget = {
  kind: "provider"
  provider: string
  utcOffsetMinutes: number
}

/** One activation returned to the popover instigator. */
export type PopoverPeekActivation = AnchoredWindowRequest<PopoverPeekTarget>

/** One generation delivered to the resident popover-peek renderer. */
export type PopoverPeekRequest = AnchoredWindowRenderRequest<PopoverPeekTarget, PopoverPeekData>

/** Native state used to bridge a listener that mounts after its first event. */
export type PopoverPeekState = AnchoredWindowState<PopoverPeekTarget>

export type PopoverPeekLifecycleEvent = AnchoredWindowLifecycleEvent<PopoverPeekTarget>

/** Fresh data loaded by the shell for one validated preview generation. */
export interface PopoverPeekData {
  kind: "provider"
  summary: ProviderUsageSummaryPayload
  live: LiveUsageSummaryPayload
}

/** Retarget the passive preview beside the popover. */
export async function showPopoverPeek(
  target: PopoverPeekTarget,
  anchor: AnchorRegion,
  initialPresentation: PopoverPeekData | null = null,
): Promise<PopoverPeekActivation> {
  if (!hasShell()) return { generation: 0, target, retargetCommitRequired: false }
  return invoke<PopoverPeekActivation>("show_popover_peek", {
    target,
    anchor,
    initialPresentation,
  })
}

/** Start the pointer-transfer grace before the passive preview clears. */
export async function hidePopoverPeek(): Promise<void> {
  if (!hasShell()) return
  await invoke("hide_popover_peek")
}

/** Read the latest preview request after the resident renderer mounts. */
export async function getPopoverPeekState(): Promise<PopoverPeekState> {
  if (!hasShell()) {
    return {
      generation: 0,
      target: null,
      rendererReady: true,
      visible: false,
      awaitingRetargetCommit: false,
      awaitingPresentation: false,
      awaitingConcealment: false,
    }
  }
  return invoke<PopoverPeekState>("get_popover_peek_state")
}

/** Read the latest preview lifecycle from the popover that owns the anchor. */
export async function getPopoverPeekAnchorState(): Promise<PopoverPeekState> {
  return getPopoverPeekState()
}

/** Load the current target through the companion's restricted shell command. */
export async function getPopoverPeekData(generation: number): Promise<PopoverPeekData> {
  if (!hasShell()) throw new Error("popover peek data requires the desktop shell")
  return invoke<PopoverPeekData>("get_popover_peek_data", { generation })
}

/** Mark the companion ready after its resident standby skeleton commits. */
export async function popoverPeekReady(generation: number): Promise<boolean> {
  if (!hasShell()) return true
  return invoke<boolean>("popover_peek_ready", { generation })
}

/** Reveal only after the current generation has committed and measured. */
export async function popoverPeekPresented(
  generation: number,
  contentHeight: number | null,
): Promise<boolean> {
  if (!hasShell()) return true
  return invoke<boolean>("popover_peek_presented", { generation, contentHeight })
}

/** Move the current native window only after its neutral target shell commits. */
export async function popoverPeekRetargetReady(
  generation: number,
  contentHeight: number | null,
): Promise<boolean> {
  if (!hasShell()) return true
  return invoke<boolean>("popover_peek_retarget_ready", { generation, contentHeight })
}

/** Confirm that React committed the cleared generation before native hiding. */
export async function popoverPeekConcealed(generation: number): Promise<boolean> {
  if (!hasShell()) return true
  return invoke<boolean>("popover_peek_concealed", { generation })
}

/** Event emitted to the resident renderer for each target or clear generation. */
export const POPOVER_PEEK_REQUEST_EVENT = "anchored-window-request"
export const POPOVER_PEEK_STATE_EVENT = "anchored-window-state"

/** Subscribe to typed target generations from the anchored-window manager. */
export async function onPopoverPeekRequest(
  handler: (request: PopoverPeekRequest) => void,
): Promise<UnlistenFn> {
  if (!hasShell()) return () => undefined
  return listen<PopoverPeekRequest>(POPOVER_PEEK_REQUEST_EVENT, (event) =>
    handler(event.payload),
  )
}

/** Subscribe to companion lifecycle changes delivered to the popover anchor. */
export async function onPopoverPeekLifecycle(
  handler: (event: PopoverPeekLifecycleEvent) => void,
): Promise<UnlistenFn> {
  if (!hasShell()) return () => undefined
  return listen<PopoverPeekLifecycleEvent>(POPOVER_PEEK_STATE_EVENT, (event) =>
    handler(event.payload),
  )
}
