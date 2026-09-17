interface Window {
  readonly __ANTIBURN_WINDOW_GENERATION__?: number
  __ANTIBURN_VISUAL_EMIT__?: (event: string, payload: unknown) => void
  __ANTIBURN_VISUAL_FAULT__?: string | null
  __ANTIBURN_VISUAL_HUD_RESIZES__?: Array<Record<string, unknown>>
  __ANTIBURN_VISUAL_HUD_DRAGS__?: string[]
}
