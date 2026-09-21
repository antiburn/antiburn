/**
 * Two short synthesised sounds for the HUD, made with the Web Audio API.
 *
 * Both are one oscillator with a pitch ramp and a gain decay. There is no
 * sample file to load. A webview with no AudioContext plays nothing.
 */

let context: AudioContext | null = null

function audio(): AudioContext | null {
  if (typeof AudioContext === "undefined") return null
  try {
    context ??= new AudioContext()
    if (context.state === "suspended") void context.resume()
    return context
  } catch {
    return null
  }
}

function tone(
  fromHz: number,
  toHz: number,
  seconds: number,
  peak: number,
  type: OscillatorType,
): void {
  const ctx = audio()
  if (!ctx) return
  const at = ctx.currentTime
  const osc = ctx.createOscillator()
  const gain = ctx.createGain()
  osc.type = type
  osc.frequency.setValueAtTime(fromHz, at)
  osc.frequency.exponentialRampToValueAtTime(toHz, at + seconds)
  gain.gain.setValueAtTime(peak, at)
  gain.gain.exponentialRampToValueAtTime(0.001, at + seconds)
  osc.connect(gain).connect(ctx.destination)
  osc.start(at)
  osc.stop(at + seconds + 0.02)
}

/** A short pop: a falling sine, for tearing the HUD off its dock. */
export function playPop(): void {
  tone(640, 140, 0.09, 0.25, "sine")
}

/** A short rising bwoop, for a nudge. */
export function playBwoop(): void {
  tone(320, 880, 0.22, 0.18, "triangle")
}
