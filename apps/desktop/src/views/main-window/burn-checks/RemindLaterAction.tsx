import * as DropdownMenu from "@radix-ui/react-dropdown-menu"
import { BellRing, Clock } from "lucide-react"
import { useState } from "react"

import type { CheckId } from "../../../lib/insightsIpc"
import {
  snoozeBurnCheck,
  snoozedCheckIds,
  unsnoozeBurnCheck,
  useSnoozedBurnChecks,
  type SnoozeDuration,
} from "../../../lib/snoozedBurnChecks"

export function RemindLaterAction({ detector }: { detector: CheckId }) {
  const [pending, setPending] = useState(false)
  const [failedAction, setFailedAction] = useState<SnoozeDuration | "unsnooze" | null>(null)
  const save = async (action: SnoozeDuration | "unsnooze") => {
    if (pending) return
    setPending(true)
    setFailedAction(null)
    try {
      if (action === "unsnooze") await unsnoozeBurnCheck(detector)
      else await snoozeBurnCheck(detector, action)
    } catch {
      setFailedAction(action)
    } finally {
      setPending(false)
    }
  }
  const snoozes = useSnoozedBurnChecks()
  const snoozed = snoozedCheckIds(snoozes.records).has(detector)
  const action = snoozed ? (
    <button
      type="button"
      disabled={pending || snoozes.status === "loading"}
      onClick={() => void save("unsnooze")}
      className="burn-check-action burn-check-reminder type-callout gap-1"
    >
      <BellRing size={12} aria-hidden="true" />
      Unsnooze
    </button>
  ) : (
    <DropdownMenu.Root>
      <DropdownMenu.Trigger asChild>
        <button
          type="button"
          disabled={pending || snoozes.status === "loading"}
          className="burn-check-action burn-check-reminder type-callout gap-1"
        >
          <Clock size={12} aria-hidden="true" />
          {pending ? "Saving snooze…" : "Snooze"}
        </button>
      </DropdownMenu.Trigger>
      <DropdownMenu.Portal>
        <DropdownMenu.Content
          className="ui-menu min-w-36"
          side="bottom"
          align="start"
          sideOffset={4}
        >
          <DropdownMenu.Item className="ui-menu-item" onSelect={() => void save("week")}>
            For 1 week
          </DropdownMenu.Item>
          <DropdownMenu.Item className="ui-menu-item" onSelect={() => void save("month")}>
            For 1 month
          </DropdownMenu.Item>
          <DropdownMenu.Item className="ui-menu-item" onSelect={() => void save("forever")}>
            Forever
          </DropdownMenu.Item>
        </DropdownMenu.Content>
      </DropdownMenu.Portal>
    </DropdownMenu.Root>
  )
  return (
    <div aria-busy={pending}>
      {action}
      {failedAction && (
        <div role="alert" className="mt-2 type-callout text-label-secondary">
          Could not {failedAction === "unsnooze" ? "unsnooze this check" : "save the snooze"}.
          <button
            type="button"
            className="burn-check-action ml-2"
            onClick={() => void save(failedAction)}
          >
            Retry {failedAction === "unsnooze" ? "unsnooze" : "snooze"}
          </button>
        </div>
      )}
    </div>
  )
}
