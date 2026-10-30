import type { PromotionDefinition, PromotionSidebar } from "../../promotions/types"

export function selectSidebarPromotion(
  catalog: readonly PromotionDefinition[],
  detectedAgents: ReadonlySet<string>,
): (PromotionDefinition & { sidebar: PromotionSidebar }) | undefined {
  const eligible = catalog.filter(
    (promo): promo is PromotionDefinition & { sidebar: PromotionSidebar } =>
      !promo.development_only &&
      promo.sidebar !== undefined &&
      (promo.sidebar.audience.type === "all" ||
        promo.sidebar.audience.agents.some((agent) => detectedAgents.has(agent))),
  )
  return eligible.sort(
    (left, right) =>
      right.sidebar.priority - left.sidebar.priority ||
      (left.id < right.id ? -1 : left.id > right.id ? 1 : 0),
  )[0]
}
