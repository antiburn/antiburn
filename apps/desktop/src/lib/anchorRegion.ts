/** Logical bounds of one target relative to its anchor window. */
export interface AnchorRegion {
  top: number
  height: number
}

export function measureAnchorRegion(element: HTMLElement): AnchorRegion {
  const bounds = element.getBoundingClientRect()
  return { top: bounds.top, height: bounds.height }
}
