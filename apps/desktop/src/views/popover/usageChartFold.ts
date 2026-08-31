/**
 * How far the list scrolls before the usage chart is fully folded away, in
 * pixels.
 */
const USAGE_CHART_FOLD_RANGE = 96

/**
 * Fold the usage chart in step with the list scroll. The wrapper's height
 * closes while the chart lifts, shrinks a little, and fades. At the top
 * every override clears and the chart keeps its natural height.
 *
 * The values are scroll-linked, so no transition applies — each frame paints
 * the offset the scroll position asks for.
 *
 * The fold gives its height back to the list, which shortens the distance
 * the list can scroll. A fold deeper than that distance leaves the reader
 * past the end, the browser pulls the offset back, and the chart opens
 * again — a list barely longer than the viewport then oscillates. So the
 * fold never takes more room than the list has left below the reader.
 */
export function foldUsageChart(wrap: HTMLDivElement | null, viewport: HTMLDivElement) {
  const chart = wrap?.firstElementChild
  if (!wrap || !(chart instanceof HTMLElement)) return
  const scrollTop = viewport.scrollTop
  const progress = Math.min(1, Math.max(0, scrollTop / USAGE_CHART_FOLD_RANGE))
  if (progress === 0) {
    wrap.style.height = ""
    chart.style.transform = ""
    chart.style.opacity = ""
    return
  }
  const height = chart.offsetHeight
  // What the list could scroll with the chart open: its present range plus
  // the room the current fold already handed over.
  const openRange = viewport.scrollHeight - viewport.clientHeight + (height - wrap.offsetHeight)
  const room = Math.min(height * progress, Math.max(0, openRange - scrollTop))
  // The wrapper keeps a whole number of pixels. Round its height up. Then the
  // fold the browser applies is never deeper than the room above permits.
  // A height that rounds down folds up to half a pixel too far. The reader
  // then sits past the end, and the browser pulls the offset back with a jump.
  const wrapHeight = Math.ceil(height - room)
  const folded = height - wrapHeight
  if (folded <= 0) {
    wrap.style.height = ""
    chart.style.transform = ""
    chart.style.opacity = ""
    return
  }
  const shown = folded / height
  wrap.style.height = `${wrapHeight}px`
  chart.style.transformOrigin = "top center"
  chart.style.transform = `translateY(${Math.round(-height * shown * 0.35)}px) scale(${(1 - 0.04 * shown).toFixed(3)})`
  chart.style.opacity = `${1 - shown}`
}
