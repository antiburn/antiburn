// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { useSyncExternalStore } from "react"

import { createExternalStore } from "../../../lib/externalStore"

/**
 * Whether the Skills & MCPs chart shows every row or only the first group.
 *
 * This is a module-level store, not component state. The reader's choice
 * stays for the life of the app process. It does not reset when the reader
 * closes one session and opens another. It resets only when the app
 * restarts.
 */
export const skillsMcpExpandedStore = createExternalStore<boolean>({ initial: false })

export function useSkillsMcpExpanded(): [boolean, (expanded: boolean) => void] {
  const expanded = useSyncExternalStore(
    skillsMcpExpandedStore.subscribe,
    skillsMcpExpandedStore.getSnapshot,
  )
  return [expanded, skillsMcpExpandedStore.set]
}
