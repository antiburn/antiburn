// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import type { LucideIcon } from 'lucide-react';
import type { ReactNode } from 'react';

/** The two documented push-button treatments. `primary` is the accent-filled
 *  variant; everything else is the default secondary. */
export type PushButtonVariant = 'secondary' | 'primary';

/** The standard push button.
 *
 *  `ui-push-button` already supplies the geometry, so this exists for the two
 *  things call sites kept re-deriving by hand: the accent-filled primary
 *  variant, and a trailing glyph (an external-link arrow, a disclosure chevron)
 *  at the 12px footnote step, decorative and out of the accessible name. */
export function PushButton({
  children,
  onClick,
  trailingIcon: TrailingIcon,
  variant = 'secondary',
  disabled = false,
  ariaLabel,
  className = '',
}: {
  children: ReactNode;
  onClick: () => void;
  trailingIcon?: LucideIcon;
  variant?: PushButtonVariant;
  disabled?: boolean;
  /** Only needed when the visible label isn't a sufficient accessible name. */
  ariaLabel?: string;
  className?: string;
}) {
  const variantClass =
    variant === 'primary'
      ? 'border-transparent bg-accent-fill text-white hover:bg-accent-hover'
      : '';
  return (
    <button
      type="button"
      aria-label={ariaLabel}
      disabled={disabled}
      onClick={onClick}
      className={`ui-push-button gap-1 whitespace-nowrap disabled:opacity-50 ${variantClass} ${className}`
        .replace(/\s+/g, ' ')
        .trimEnd()}
    >
      {children}
      {TrailingIcon && (
        <TrailingIcon size={12} strokeWidth={2} aria-hidden="true" className="shrink-0" />
      )}
    </button>
  );
}
