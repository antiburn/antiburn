export async function confirm(message: string): Promise<boolean> {
  window.__ANTIBURN_VISUAL_CONFIRMATIONS__ ??= []
  window.__ANTIBURN_VISUAL_CONFIRMATIONS__.push(message)
  return new URLSearchParams(window.location.search).get("confirm") === "accept"
}

declare global {
  interface Window {
    __ANTIBURN_VISUAL_CONFIRMATIONS__?: string[]
  }
}

export async function open(): Promise<string | null> {
  return null
}

export async function save(): Promise<string | null> {
  return null
}
