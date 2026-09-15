/** The restricted transport injected into the native macOS preview webview. */
export interface NativePeekBridge {
  invoke(command: string, args?: object): Promise<unknown>
  listen(event: string, callback: (payload: unknown) => void): Promise<() => void>
}

declare global {
  interface Window {
    readonly __ANTIBURN_NATIVE_PEEK__?: NativePeekBridge
  }
}

/** Return the native preview transport when this page runs in its WKWebView. */
export function nativePeekBridge(): NativePeekBridge | null {
  return window.__ANTIBURN_NATIVE_PEEK__ ?? null
}
