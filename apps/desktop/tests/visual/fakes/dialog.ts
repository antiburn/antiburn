export async function confirm(): Promise<boolean> {
  return false
}

export async function open(): Promise<string | null> {
  // The Claude profiles fixture picks one synthetic folder.
  return new URLSearchParams(window.location.search).get("profiles") === "1"
    ? "/Users/fixture/.claude-extra"
    : null
}

export async function save(): Promise<string | null> {
  return null
}
