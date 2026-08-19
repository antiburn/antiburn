// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { useCallback, useRef } from 'react';

import { useExternalSubscription } from './useExternalSubscription';

export type HoverIntent = {
  /** Replace any pending timer with a new one that runs `run` after `delayMs`. */
  schedule: (run: () => void, delayMs: number) => void;
  /** Clear the pending timer, if any, without running it. */
  cancel: () => void;
};

/**
 * A single pending timer, scoped to one component instance.
 *
 * The recurring "hover intent" pattern — delay an action so a pointer merely
 * passing through doesn't trigger it, cancel the delay if the pointer leaves
 * first — needs exactly one timer in flight at a time and needs it cleared on
 * unmount so it never fires into a dead component. `schedule` and `cancel` are
 * stable identities (a ref for the timer handle, `useCallback` for the
 * functions), so a caller can hand them straight to `onMouseEnter` /
 * `onMouseLeave` without them changing every render.
 */
export function useHoverIntent(): HoverIntent {
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);

  const cancel = useCallback(() => {
    if (timer.current !== null) {
      clearTimeout(timer.current);
      timer.current = null;
    }
  }, []);

  const schedule = useCallback(
    (run: () => void, delayMs: number) => {
      cancel();
      timer.current = setTimeout(() => {
        timer.current = null;
        run();
      }, delayMs);
    },
    [cancel],
  );

  // Unmount cleanup, via the sanctioned lifecycle primitive rather than an
  // effect: subscribe attaches nothing (there is nothing to listen to — the
  // timer is driven entirely by `schedule`/`cancel`) and its cleanup is
  // exactly `cancel`, run when the component goes away.
  useExternalSubscription(useCallback(() => cancel, [cancel]));

  return { schedule, cancel };
}
