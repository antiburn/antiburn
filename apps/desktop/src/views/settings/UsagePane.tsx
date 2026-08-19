// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { useState, useSyncExternalStore } from "react"

import { Card } from "../../components/ui/Card"
import { Pane } from "../../components/ui/Pane"
import { Row } from "../../components/ui/Row"
import { SectionGroup } from "../../components/ui/SectionGroup"
import { ToggleRow } from "../../components/ui/ToggleRow"
import { createExternalStore } from "../../lib/externalStore"
import { getLiveUsage, EMPTY_LIVE_USAGE } from "../../lib/ipc"
import { liveSourceNote } from "../../lib/presentation/liveUsage"
import type { AppSettingsController } from "./useAppSettings"

/**
 * Usage: where the plan limits come from, and the one switch that turns it
 * off.
 *
 * The switch below is on by default: antiburn asks each provider directly for
 * your current usage, about every ten minutes, using the credentials your own
 * coding tools already hold, entirely over your own connection. That is
 * ordinary traffic — your own usage, from a provider you already use, with a
 * credential you already hold — so it runs without asking first, the same way
 * every other local reading in this app does. The switch exists for a reader
 * who wants none of it: turn it off and this pane has nothing to show, no
 * request is made, and no credential is read.
 *
 * The copy names both things the switch controls — it is what makes readings
 * possible at all, *and* it is what lets milestone notifications fire —
 * because a switch with two consequences has to say both or a reader turning
 * it off for one reason is surprised by the other.
 */

export type UsagePaneProps = AppSettingsController

/** What a failed source means, phrased as something a reader could act on. */
function errorNote(category: string): string {
  switch (category) {
    case "authentication":
      return "antiburn could not sign in to read your plan usage. Sign in again with your coding tool, then reopen this view."
    case "rateLimited":
      return "Your provider asked antiburn to slow down. It will try again later."
    case "schema":
      return "Your provider reported usage in a shape antiburn does not recognise."
    default:
      return "antiburn could not reach your provider for usage. It will try again later."
  }
}

export function UsagePane({ settings, update }: UsagePaneProps) {
  // One read on open, and one after the switch moves (from the ToggleRow's
  // onChange below). Not a subscription: this pane is a place a reader visits
  // deliberately, and a limit figure that ticked over while they were looking
  // at a preference would be noise. Per-instance rather than a module
  // singleton: each mount gets its own read, matching the effect this
  // replaced.
  const [store] = useState(() =>
    createExternalStore({
      initial: EMPTY_LIVE_USAGE,
      load: () => getLiveUsage().catch(() => EMPTY_LIVE_USAGE),
    }),
  )
  const live = useSyncExternalStore(store.subscribe, store.getSnapshot)

  const on = settings?.liveUsageEnabled ?? false

  return (
    <Pane title="Usage">
      <SectionGroup title="Keeping limits current">
        <Card>
          <ToggleRow
            label="Keep my plan limits current"
            description="Asks each provider directly for your current usage, about every ten minutes, using the credentials your own coding tools already have — that's your own connection, made as you; no antiburn server is involved. When a provider can't be reached directly, antiburn falls back to asking your coding tool's own local process the same question. Turning this off also stops usage milestone notifications, since they need readings that keep moving."
            checked={on}
            onChange={(next) =>
              void Promise.resolve(update({ liveUsageEnabled: next })).then(() =>
                store.refresh(),
              )
            }
          />
          <Row
            label="With this off"
            description="antiburn makes none of these requests and shows no plan limits at all."
          />
        </Card>
      </SectionGroup>

      <SectionGroup title="What antiburn can currently see">
        <Card>
          {live.providers.length === 0 && live.errors.length === 0 && (
            <Row
              label="No plan limits found"
              description={
                on
                  ? "No provider credentials were found on this machine yet. Sign in with a coding tool and this fills in."
                  : "Turn the switch above back on to ask your providers for current plan limits."
              }
            />
          )}
          {live.providers.map((provider) => (
            <Row
              key={provider.provider}
              label={provider.displayName}
              description={`${provider.sourceLabel}. ${provider.windows.length} limit${
                provider.windows.length === 1 ? "" : "s"
              } reported.`}
              trailing={
                <span className="type-caption tabular-nums text-label-tertiary">
                  {liveSourceNote(provider)}
                </span>
              }
            />
          ))}
          {live.errors.map((error) => (
            <Row
              key={error.source}
              label="Could not read usage"
              description={errorNote(error.category)}
            />
          ))}
        </Card>
      </SectionGroup>
    </Pane>
  )
}
