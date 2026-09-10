/**
 * The last stagger step `hud.css` defines. A provider's meters turn on one
 * step apart, 100 ms each, and the on phase is 1.5 s long. A row past this
 * step shares the last step, so it still turns on before the rows turn off.
 */
const LED_BLINK_MAX_STEP = 5

/**
 * The `data-led-step` value for a blinking segment at `row` within its
 * provider, or `undefined` for the first row, which uses the base flash colour.
 */
export function ledBlinkStep(row: number): number | undefined {
  if (row <= 0) return undefined
  return Math.min(row, LED_BLINK_MAX_STEP)
}
