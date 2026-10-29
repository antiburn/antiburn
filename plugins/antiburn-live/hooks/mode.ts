export type LiveMode = 'usage' | 'checks'

export function liveMode(value: string | undefined): LiveMode {
  return value === 'checks' ? 'checks' : 'usage'
}
