// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import type { ReactNode } from "react"

import { cn } from "../../lib/cn"

/**
 * A link rendered inline within a sentence.
 *
 * A `button`, not an `<a>`: an outbound link has to land in the user's browser
 * rather than navigating the webview, and a real href in a desktop window is a
 * navigation hazard. This primitive stays callback-only — the app shell owns
 * the effect of opening a URL, so the same component works in a test, in a
 * browser, and in the packaged app.
 *
 * Inherits the surrounding prose size so it sits on the same baseline; the
 * underline is what marks it, not a color change, which keeps it legible on
 * both the window and card surfaces.
 */
export function InlineLink({
  children,
  onClick,
  className = "",
}: {
  children: ReactNode
  onClick: () => void
  className?: string
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      className={cn(
        "rounded-control text-label underline decoration-label-tertiary underline-offset-2 transition-colors duration-[var(--duration-fast)] ease-out hover:decoration-label",
        className,
      )}
    >
      {children}
    </button>
  )
}
