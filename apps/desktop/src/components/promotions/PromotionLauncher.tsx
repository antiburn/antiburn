import { ArrowRight, Megaphone } from "lucide-react"
import { useState, useSyncExternalStore, type Ref } from "react"
import { renderAgentIcon } from "../../lib/agentIcon"
import { createExternalStore } from "../../lib/externalStore"
import { EMPTY_LIVE_USAGE, getLiveUsage, onLiveUsageChanged } from "../../lib/ipc"
import { agentStatus, meterForAgent } from "../../lib/presentation/agentStatus"
import { AGENT_SLUGS } from "../../lib/presentation/agents"
import type { PromotionSession } from "../../lib/promotions/PromotionSession"
import { selectSidebarPromotion } from "../../lib/promotions/selectSidebarPromotion"
import {
  agentSessionCounts,
  subscribeAgentSessionCounts,
} from "../../views/main-window/overview/stepSettings/agentSessionCounts"

export function PromotionLauncher({
  session,
  onOpen,
  buttonRef,
}: {
  session: PromotionSession
  onOpen: () => void
  buttonRef?: Ref<HTMLButtonElement>
}) {
  const counts = useSyncExternalStore(subscribeAgentSessionCounts, agentSessionCounts)
  const [liveStore] = useState(() =>
    createExternalStore({
      initial: EMPTY_LIVE_USAGE,
      load: () => getLiveUsage().catch(() => EMPTY_LIVE_USAGE),
      subscribe: onLiveUsageChanged,
    }),
  )
  const live = useSyncExternalStore(liveStore.subscribe, liveStore.getSnapshot)
  useSyncExternalStore(session.subscribe, session.getSnapshot)
  const detectedAgents = new Set(
    AGENT_SLUGS.filter((agent) => {
      const count = counts.find((entry) => entry.agent === agent)?.sessions ?? 0
      return agentStatus(count, meterForAgent(agent, live.meters)).found
    }),
  )
  const promo = selectSidebarPromotion(session.catalog, detectedAgents)
  if (!promo) return null
  const { sidebar } = promo
  return (
    <button
      ref={buttonRef}
      type="button"
      className="promotion-launcher flex w-full flex-col gap-1 rounded-popover p-3 text-left text-label"
      aria-label={[sidebar.title, sidebar.badge?.toLowerCase()].filter(Boolean).join(", ")}
      aria-haspopup="dialog"
      onClick={(event) => {
        if (session.openFromLauncher(promo.id, event.detail > 0)) onOpen()
      }}
    >
      <span className="flex w-full items-center gap-2 type-body">
        <span aria-hidden="true" className="shrink-0">
          {sidebar.icon === "megaphone" ? (
            <Megaphone size={14} className="text-label-secondary" />
          ) : (
            renderAgentIcon(sidebar.icon, 14)
          )}
        </span>
        <span className="min-w-0 font-medium">{sidebar.title}</span>
        {sidebar.badge && (
          <span
            className="promotion-launcher-badge shrink-0 rounded-control bg-surface-secondary px-1 type-caption font-medium text-label-secondary"
            aria-hidden="true"
          >
            {sidebar.badge}
          </span>
        )}
      </span>
      <span className="type-caption text-label-secondary">
        {sidebar.description}
        {sidebar.link_label && (
          <>
            {" "}
            <span className="inline-flex items-center gap-1 whitespace-nowrap font-medium text-label-secondary">
              {sidebar.link_label} <ArrowRight size={12} aria-hidden="true" />
            </span>
          </>
        )}
      </span>
    </button>
  )
}
