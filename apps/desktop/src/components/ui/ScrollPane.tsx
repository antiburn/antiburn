// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import * as ScrollArea from '@radix-ui/react-scroll-area';
import type { ReactNode, Ref } from 'react';

function syncTopEdgeFade(viewport: HTMLDivElement) {
  const shouldFade = viewport.scrollTop > 1;
  if (shouldFade === viewport.hasAttribute('data-scroll-edge-top')) return;

  if (shouldFade) {
    viewport.setAttribute('data-scroll-edge-top', 'active');
  } else {
    viewport.removeAttribute('data-scroll-edge-top');
  }
}

function assignRef<T>(ref: Ref<T> | undefined, value: T | null) {
  if (typeof ref === 'function') ref(value);
  else if (ref) ref.current = value;
}

/** A scroll container wired to the design foundation's scrollbar styling.
 *
 *  `ui-scroll-viewport` is load-bearing, not cosmetic: on macOS it forces
 *  `overflow-y: scroll` (see styles/platform-controls.css) so scrollability
 *  never depends on Radix's ResizeObserver measurement, which latches to "no
 *  overflow" across a window hide/show cycle. */
export function ScrollPane({
  children,
  className = '',
  viewportClassName = '',
  viewportRef,
  topEdgeFade = false,
}: {
  children: ReactNode;
  className?: string;
  viewportClassName?: string;
  viewportRef?: Ref<HTMLDivElement>;
  /** Fade scrolling content into the viewport's top edge after it leaves the
   *  initial position. The scrollbar remains outside the mask. */
  topEdgeFade?: boolean;
}) {
  const assignViewportRef = (node: HTMLDivElement | null) => {
    if (node && topEdgeFade) syncTopEdgeFade(node);
    assignRef(viewportRef, node);
  };

  return (
    <ScrollArea.Root className={`flex-1 overflow-hidden ${className}`.trimEnd()}>
      <ScrollArea.Viewport
        ref={assignViewportRef}
        onScroll={topEdgeFade ? (event) => syncTopEdgeFade(event.currentTarget) : undefined}
        className={`ui-scroll-viewport h-full${topEdgeFade ? ' scroll-edge-fade-top' : ''} ${viewportClassName}`.trimEnd()}
      >
        {children}
      </ScrollArea.Viewport>
      <ScrollArea.Scrollbar className="ui-scrollbar" orientation="vertical">
        <ScrollArea.Thumb className="ui-scrollbar-thumb" />
      </ScrollArea.Scrollbar>
    </ScrollArea.Root>
  );
}
