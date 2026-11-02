export {
  BRAND_MARK_SVG,
  meterDescription,
  responsiveMeterSvg,
  RESPONSIVE_FRAME_WIDTH,
  usageStatusSvg,
  secondaryTextSvg,
} from "./usage-meter-graphics"
import { meterSvg as renderMeter } from "./usage-meter-graphics"
import type { UsageWindow } from "./usage"
export function meterSvg(
  window: UsageWindow,
  width: number,
  now: number,
): string {
  return renderMeter(window, width, now, false, true, {
    layout: "inline",
    snapMarker: true,
  })
}
