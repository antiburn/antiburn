import * as DropdownMenu from "@radix-ui/react-dropdown-menu"
import { BellRing, Clock } from "lucide-react"

import type { BurnCheckDetectorId } from "../../../lib/insightsIpc"
import {
  snoozeBurnCheck,
  snoozedDetectorIds,
  unsnoozeBurnCheck,
  useSnoozedBurnChecks,
  type SnoozeDuration,
} from "../../../lib/snoozedBurnChecks"

export function RemindLaterAction({ detector }: { detector: BurnCheckDetectorId }) {
  const snooze = (duration: SnoozeDuration) => void snoozeBurnCheck(detector, duration)
  const snoozed = snoozedDetectorIds(useSnoozedBurnChecks()).has(detector)
  if (snoozed)
    return (
      <button
        type="button"
        onClick={() => void unsnoozeBurnCheck(detector)}
        className="burn-check-action burn-check-reminder type-callout gap-1"
      >
        <BellRing size={12} aria-hidden="true" />
        Unsnooze
      </button>
    )
  return (
    <DropdownMenu.Root>
      <DropdownMenu.Trigger asChild>
        <button
          type="button"
          className="burn-check-action burn-check-reminder type-callout gap-1"
        >
          <Clock size={12} aria-hidden="true" />
          Snooze
        </button>
      </DropdownMenu.Trigger>
      <DropdownMenu.Portal>
        <DropdownMenu.Content
          className="ui-menu min-w-36"
          side="bottom"
          align="start"
          sideOffset={4}
        >
          <DropdownMenu.Item className="ui-menu-item" onSelect={() => snooze("week")}>
            For 1 week
          </DropdownMenu.Item>
          <DropdownMenu.Item className="ui-menu-item" onSelect={() => snooze("month")}>
            For 1 month
          </DropdownMenu.Item>
          <DropdownMenu.Item className="ui-menu-item" onSelect={() => snooze("forever")}>
            Forever
          </DropdownMenu.Item>
        </DropdownMenu.Content>
      </DropdownMenu.Portal>
    </DropdownMenu.Root>
  )
}
