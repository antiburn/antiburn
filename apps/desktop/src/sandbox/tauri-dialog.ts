/**
 * Sandbox replacement for `@tauri-apps/plugin-dialog`.
 *
 * `confirm` uses the browser dialog. The file pickers have no browser
 * equivalent here, so `save` and `open` report that the reader cancelled.
 */

interface ConfirmOptions {
  title?: string
}

export async function confirm(
  message: string,
  options?: string | ConfirmOptions,
): Promise<boolean> {
  const title = typeof options === "string" ? options : options?.title
  return window.confirm(title ? `${title}\n\n${message}` : message)
}

export async function save(): Promise<string | null> {
  return null
}

export async function open(): Promise<string | string[] | null> {
  return null
}
