// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { useEffect, useRef, useState } from 'react';

export interface ActivityGroupPinning {
  /** Callback ref for the scrolling viewport the groups live in. */
  assignViewportRef: (node: HTMLDivElement | null) => void;
  /** Callback-ref factory for each in-flow group heading, keyed by its label. */
  registerHeading: (label: string) => (node: HTMLHeadingElement | null) => void;
  /** The day label to show in the sticky header, or null when there are none. */
  pinnedLabel: string | null;
  /** Scroll back to the top and un-pin. */
  resetViewport: () => void;
}

/**
 * Track which day heading a scrolling activity list is currently under, so the
 * list can paint one sticky label above the viewport.
 *
 * The scroll machinery belongs to the list, not to any one row kind: it needs
 * only the ordered day labels and the headings rendered for them.
 */
export function useActivityGroupPinning(
  groupLabels: string[],
  externalViewportRef?: (node: HTMLDivElement | null) => void,
): ActivityGroupPinning {
  const viewportRef = useRef<HTMLDivElement | null>(null);
  const headingRefs = useRef(new Map<string, HTMLHeadingElement>());
  const [pinnedGroupLabel, setPinnedGroupLabel] = useState<string | null>(null);

  // NUL-joined because day labels contain spaces, so the separator has to be a
  // character a label can never hold.
  const groupLabelsSignature = groupLabels.join('\u0000');

  function resetViewport() {
    setPinnedGroupLabel(null);
    const viewport = viewportRef.current;
    if (viewport) viewport.scrollTop = 0;
  }

  const assignViewportRef = (node: HTMLDivElement | null) => {
    viewportRef.current = node;
    externalViewportRef?.(node);
  };

  const registerHeading = (label: string) => (node: HTMLHeadingElement | null) => {
    if (node) headingRefs.current.set(label, node);
    else headingRefs.current.delete(label);
  };

  useEffect(() => {
    const viewport = viewportRef.current;
    if (!viewport || !groupLabelsSignature) return;

    const labels = groupLabelsSignature.split('\u0000');
    let animationFrame: number | null = null;

    const updatePinnedGroup = () => {
      animationFrame = null;
      const viewportTop = viewport.getBoundingClientRect().top + 0.5;
      let nextLabel = labels[0];

      for (const label of labels.slice(1)) {
        const heading = headingRefs.current.get(label);
        if (!heading || heading.getBoundingClientRect().top > viewportTop) break;
        nextLabel = label;
      }

      setPinnedGroupLabel((current) => (current === nextLabel ? current : (nextLabel ?? null)));
    };

    // Coalesce scroll events onto one frame: a trackpad fling fires far more
    // often than the display refreshes, and the answer only changes per frame.
    const scheduleUpdate = () => {
      if (animationFrame !== null) return;
      animationFrame = requestAnimationFrame(updatePinnedGroup);
    };

    viewport.addEventListener('scroll', scheduleUpdate, { passive: true });
    if (viewport.scrollTop > 0) scheduleUpdate();

    return () => {
      viewport.removeEventListener('scroll', scheduleUpdate);
      if (animationFrame !== null) cancelAnimationFrame(animationFrame);
    };
  }, [groupLabelsSignature]);

  return {
    assignViewportRef,
    registerHeading,
    // A pinned label whose group no longer exists (the list changed under it)
    // falls back to the first day rather than lingering.
    pinnedLabel: groupLabels.includes(pinnedGroupLabel ?? '')
      ? pinnedGroupLabel
      : (groupLabels[0] ?? null),
    resetViewport,
  };
}
