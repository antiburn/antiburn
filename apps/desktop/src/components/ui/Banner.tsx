// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { X, type LucideIcon } from 'lucide-react';

/** How loudly a banner reads. */
export type BannerTone = 'warning' | 'info';

/**
 * One line of attention: a glyph, a short message, an optional action, and a
 * dismissal.
 *
 * The rules this shape enforces, because they are what keeps a banner from
 * becoming noise:
 *
 * - **It is always dismissible.** A banner a reader cannot silence is a banner
 *   that trains them to ignore the space it occupies.
 * - **It has at most one action**, and that action is a place to go rather than
 *   a thing to guess at — "Review" opens the pane that can fix it, "Retry"
 *   tries the failed thing again.
 * - **It is one line of prose.** Anything that needs a paragraph belongs in a
 *   pane, not over a list someone is trying to read.
 *
 * Rendered as `role="status"` with a polite live region: something a reader
 * should learn about, not something that interrupts what they are reading.
 */
export function Banner({
  tone = 'warning',
  icon: Icon,
  message,
  actionLabel,
  onAction,
  onDismiss,
  dismissLabel = 'Dismiss',
  className = '',
}: {
  tone?: BannerTone;
  /** Decorative: the message carries the meaning. */
  icon?: LucideIcon;
  message: string;
  /** Omit both action props for a banner that only reports. */
  actionLabel?: string;
  onAction?: () => void;
  onDismiss: () => void;
  /** Accessible name of the dismiss control, when "Dismiss" is too vague. */
  dismissLabel?: string;
  className?: string;
}) {
  // Tinted fills rather than a solid status colour: the popover's surfaces are
  // translucent, and a saturated band across one would read as a different
  // window rather than a note inside this one.
  const toneClass =
    tone === 'warning'
      ? 'bg-system-orange-tint/12 text-system-orange'
      : 'bg-surface-secondary text-label-secondary';

  return (
    <div
      role="status"
      aria-live="polite"
      data-testid="banner"
      // 8px horizontal padding, so a caller that lays banners out on the same
      // 8px gutter as a list gets text starting on the list's own left edge.
      className={`flex items-start gap-2 rounded-control px-2 py-1.5 ${toneClass} ${className}`.trimEnd()}
    >
      {Icon && (
        <Icon size={12} strokeWidth={2} aria-hidden="true" className="mt-0.5 shrink-0" />
      )}
      <p className="min-w-0 flex-1 type-caption text-pretty text-label">{message}</p>
      {actionLabel && onAction && (
        <button
          type="button"
          onClick={onAction}
          className="shrink-0 rounded-control px-1.5 type-caption font-medium text-label hover:bg-surface-hover"
        >
          {actionLabel}
        </button>
      )}
      <button
        type="button"
        onClick={onDismiss}
        aria-label={dismissLabel}
        className="-mr-0.5 mt-px inline-flex h-4 w-4 shrink-0 items-center justify-center rounded-control text-label-tertiary hover:bg-surface-hover hover:text-label-secondary"
      >
        <X size={11} strokeWidth={2.5} aria-hidden="true" />
      </button>
    </div>
  );
}
