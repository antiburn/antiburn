/**
 * Dev-only perf trace ring buffer, for the Quota freeze investigation.
 * Safari's Timelines recorder crashes this app, so this gives Dave a plain
 * console-pasteable timeline instead. Every export is a no-op outside
 * `import.meta.env.DEV`, so a production build carries no observer, no ring
 * buffer growth, and no console output.
 */

/** One recorded moment: a wall clock reading plus a monotonic clock reading,
 *  so entries stay orderable even across a system clock change. */
interface TraceEntry {
  t: number
  wall: number
  name: string
  [field: string]: unknown
}

declare global {
  interface Window {
    __antiburnTrace?: {
      dump: () => TraceEntry[]
      clear: () => void
      json: () => string
    }
  }
}

const MAX_ENTRIES = 2000
const STALL_INTERVAL_MS = 250
const STALL_THRESHOLD_MS = 200

const entries: TraceEntry[] = []

function record(name: string, fields: Record<string, unknown>): void {
  entries.push({ t: performance.now(), wall: Date.now(), name, ...fields })
  if (entries.length > MAX_ENTRIES) entries.shift()
  // aislop-ignore-next-line ai-slop/console-leftover -- dev-only trace sink, gated on import.meta.env.DEV
  console.debug("[trace]", name, fields)
}

/** Append one instant event to the ring buffer. No-op outside DEV. */
export function traceEvent(name: string, fields: Record<string, unknown> = {}): void {
  if (!import.meta.env.DEV) return
  record(name, fields)
}

/** Run a synchronous block and record its wall-clock duration. Rethrows
 *  whatever `run` throws, after recording the failed span. */
export function traceSpan<T>(name: string, fields: Record<string, unknown>, run: () => T): T {
  if (!import.meta.env.DEV) return run()
  const start = performance.now()
  let result: T
  try {
    result = run()
  } catch (error) {
    record(name, { ...fields, durationMs: performance.now() - start, error: true })
    throw error
  }
  record(name, { ...fields, durationMs: performance.now() - start })
  return result
}

/** Same as [[traceSpan]], for a promise-returning block. */
export async function traceAsync<T>(
  name: string,
  fields: Record<string, unknown>,
  run: () => Promise<T>,
): Promise<T> {
  if (!import.meta.env.DEV) return run()
  const start = performance.now()
  try {
    const result = await run()
    record(name, { ...fields, durationMs: performance.now() - start })
    return result
  } catch (error) {
    record(name, { ...fields, durationMs: performance.now() - start, error: true })
    throw error
  }
}

interface LongTaskAttribution {
  name: string
}

interface LongTaskEntry extends PerformanceEntry {
  attribution?: LongTaskAttribution[]
}

function installStallFallback(): void {
  let last = performance.now()
  setInterval(() => {
    const now = performance.now()
    const lagMs = now - last - STALL_INTERVAL_MS
    last = now
    if (lagMs > STALL_THRESHOLD_MS) record("main_thread_stall", { lagMs })
  }, STALL_INTERVAL_MS)
}

let longTaskObserverInstalled = false

/** Watch for main-thread stalls. Installs a real `longtask`
 *  `PerformanceObserver` when the WKWebView build supports one, else a cheap
 *  polling fallback. Call once, at app start. No-op outside DEV. */
export function installLongTaskObserver(): void {
  if (!import.meta.env.DEV || longTaskObserverInstalled) return
  longTaskObserverInstalled = true
  const supported = PerformanceObserver.supportedEntryTypes.includes("longtask")
  if (!supported) {
    record("longtask_unsupported", {})
    installStallFallback()
    return
  }
  const observer = new PerformanceObserver((list) => {
    for (const entry of list.getEntries() as LongTaskEntry[]) {
      record("longtask", {
        durationMs: entry.duration,
        attribution: entry.attribution?.map((item) => item.name) ?? [],
      })
    }
  })
  observer.observe({ entryTypes: ["longtask"] })
}

if (import.meta.env.DEV) {
  window.__antiburnTrace = {
    dump: () => entries.slice(),
    clear: () => {
      entries.length = 0
    },
    json: () => JSON.stringify(entries),
  }
}
