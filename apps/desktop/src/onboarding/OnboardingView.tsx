import { useState, useSyncExternalStore } from "react"
import { CloudOnboarding } from "./CloudOnboarding"
import appIcon from "../assets/app-icon.png"
import { renderAgentIcon } from "../lib/agentIcon"
import "./onboarding.css"

type OnboardingStep =
  "welcome" | "sessions" | "limits" | "hud" | "checks" | "cloud" | "cloudUnavailable"
export interface OnboardingViewProps {
  step: OnboardingStep
  onStepChange: (step: OnboardingStep) => void
  onComplete: () => void
  busy?: boolean
  error?: string | null | undefined
  hudSupported?: boolean
  hudVariant?: "notch" | "floating" | "unsupported"
  visible?: boolean
}

const order: OnboardingStep[] = ["welcome", "sessions", "limits", "hud", "checks", "cloud"]
const slides = {
  sessions: {
    title: "Understand what each session uses",
    body: "Bring your coding sessions together and explore the tokens, tools and estimated cost behind each conversation.",
    support: "Cost is an API-equivalent estimate, not necessarily your bill.",
    times: [700, 1250, 3550, 4050],
    duration: 7300,
  },
  limits: {
    title: "See your limits—and when they reset",
    body: "Check your providers’ usage and reset times from the menu bar, without interrupting your work.",
    support: "Provider allowances are separate from session cost.",
    times: [600, 1000, 2300],
    duration: 5700,
  },
  hud: {
    title: "Stay informed without losing focus",
    body: "Hover to check your limits, and let Antiburn’s notifications flag important changes while you stay focused.",
    support: "Available on supported platforms. Manage the HUD and notifications in Settings.",
    times: [500, 750, 2950, 3450],
    duration: 4300,
  },
  checks: {
    title: "See what you could improve",
    body: "Explore checks that highlight potential waste, understand each finding and open the session behind it.",
    support: "Some checks require additional setup.",
    times: [1400, 2000, 2800],
    duration: 4000,
  },
  cloud: {
    title: "Upload all your sessions, across devices",
    body: "Upload sessions from your chosen folders to antiburn cloud, and explore their usage and insights across your devices.",
    support: "antiburn cloud is optional. You can keep using Antiburn locally.",
    times: [700, 1100, 1600, 2200, 2900],
    duration: 4200,
  },
} as const

type TeachingStep = keyof typeof slides

export function createPlayback(
  times: readonly number[],
  duration: number,
  initiallyVisible = true,
) {
  let visible = initiallyVisible
  let phase = 0
  let snapshot = 0
  let elapsed = 0
  let start = 0
  let timer: number | undefined
  let media: MediaQueryList | undefined
  const listeners = new Set<() => void>()
  const notify = () => listeners.forEach((listener) => listener())
  const stop = () => {
    if (timer !== undefined) cancelAnimationFrame(timer)
    timer = undefined
  }
  const update = () => {
    stop()
    if (media?.matches) {
      phase = times.length
      snapshot = phase
      elapsed = duration
      notify()
      return
    }
    if (document.hidden || !visible) return
    start = Date.now()
    const tick = () => {
      const current = elapsed + Date.now() - start
      const next = times.filter((time) => current >= time).length
      phase = next
      snapshot = phase + Math.min(0.999, current / (duration + 1))
      notify()
      if (current < duration) timer = requestAnimationFrame(tick)
    }
    tick()
  }
  const visibility = () => {
    if (document.hidden || !visible) {
      if (timer !== undefined) elapsed += Date.now() - start
      stop()
    } else update()
  }
  return {
    subscribe(listener: () => void) {
      listeners.add(listener)
      if (listeners.size === 1) {
        media = window.matchMedia("(prefers-reduced-motion: reduce)")
        media.addEventListener("change", update)
        document.addEventListener("visibilitychange", visibility)
        update()
      }
      return () => {
        listeners.delete(listener)
        if (!listeners.size) {
          stop()
          media?.removeEventListener("change", update)
          document.removeEventListener("visibilitychange", visibility)
        }
      }
    },
    setVisible(value: boolean) {
      if (visible === value) return
      visible = value
      visibility()
    },
    seek(step: number) {
      stop()
      elapsed = step === 0 ? 0 : (times[step - 1] ?? duration)
      phase = times.filter((time) => elapsed >= time).length
      snapshot = phase + Math.min(0.999, elapsed / (duration + 1))
      notify()
      update()
    },
    getSnapshot: () => snapshot,
    getElapsed: () =>
      Math.min(duration, elapsed + (timer === undefined ? 0 : Date.now() - start)),
  }
}

function subscribeTheme(listener: () => void) {
  const observer = new MutationObserver(listener)
  observer.observe(document.documentElement, {
    attributes: true,
    attributeFilter: ["data-theme"],
  })
  const media = window.matchMedia("(prefers-color-scheme: dark)")
  media.addEventListener("change", listener)
  return () => {
    observer.disconnect()
    media.removeEventListener("change", listener)
  }
}
function currentDemoTheme() {
  const theme = document.documentElement.dataset.theme
  return theme === "dark" ||
    (theme !== "light" && window.matchMedia("(prefers-color-scheme: dark)").matches)
    ? "dark"
    : "light"
}

function subscribeDocumentVisibility(listener: () => void) {
  document.addEventListener("visibilitychange", listener)
  return () => document.removeEventListener("visibilitychange", listener)
}
function documentVisible() {
  return !document.hidden
}

function Demo({
  step,
  hudSupported,
  hudVariant,
  visible,
}: {
  step: TeachingStep
  hudSupported: boolean
  hudVariant?: "notch" | "floating" | "unsupported"
  visible: boolean
}) {
  const slide = slides[step]
  const hostVisible = useSyncExternalStore(
    subscribeDocumentVisibility,
    documentVisible,
    () => true,
  )
  const effectiveVisible = visible && hostVisible
  const theme = useSyncExternalStore(subscribeTheme, currentDemoTheme, currentDemoTheme)
  const [playback] = useState(() => createPlayback(slide.times, slide.duration, false))
  const [channel] = useState(() => {
    let current: HTMLIFrameElement | null = null
    let status: "loading" | "ready" | "error" = "loading"
    let timeout: ReturnType<typeof setTimeout> | undefined
    let channelVisible = true
    const listeners = new Set<() => void>()
    const setStatus = (value: typeof status) => {
      if (timeout !== undefined) clearTimeout(timeout)
      timeout = undefined
      status = value
      listeners.forEach((listener) => listener())
    }
    const syncTimeout = () => {
      if (!channelVisible && timeout !== undefined) {
        clearTimeout(timeout)
        timeout = undefined
      }
      if (channelVisible && listeners.size && status === "loading" && timeout === undefined)
        timeout = setTimeout(() => setStatus("error"), 5000)
    }
    const receive = (event: MessageEvent) => {
      if (
        !current ||
        event.source !== current.contentWindow ||
        event.data?.source !== "antiburn-onboarding-demo"
      )
        return
      if (event.data.type === "ready") setStatus("ready")
      else if (event.data.type === "error") setStatus("error")
      else if (
        event.data.type === "step" &&
        Number.isInteger(event.data.step) &&
        event.data.step >= 0 &&
        event.data.step <= slide.times.length
      )
        playback.seek(event.data.step)
    }
    return {
      subscribe(listener: () => void) {
        listeners.add(listener)
        syncTimeout()
        return () => {
          listeners.delete(listener)
          if (!listeners.size && timeout !== undefined) {
            clearTimeout(timeout)
            timeout = undefined
          }
        }
      },
      setVisible(value: boolean) {
        channelVisible = value
        syncTimeout()
      },
      getSnapshot: () => status,
      fail: () => setStatus("error"),
      attach(node: HTMLIFrameElement | null) {
        if (current) window.removeEventListener("message", receive)
        current = node
        if (current) window.addEventListener("message", receive)
      },
    }
  })
  const status = useSyncExternalStore(
    channel.subscribe,
    channel.getSnapshot,
    channel.getSnapshot,
  )
  const phase = useSyncExternalStore(
    playback.subscribe,
    playback.getSnapshot,
    playback.getSnapshot,
  )
  const frame = (node: HTMLIFrameElement | null) => {
    channel.setVisible(effectiveVisible)
    playback.setVisible(effectiveVisible && status === "ready")
    channel.attach(node)
    if (!node) return
    const send = () =>
      node.contentWindow?.postMessage(
        {
          source: "antiburn-onboarding",
          state: {
            slide: ["sessions", "limits", "hud", "checks", "cloud"].indexOf(step),
            step: Math.floor(phase),
            elapsed: playback.getElapsed(),
            transferProgress: Math.max(0, Math.min(1, (playback.getElapsed() - 1600) / 1300)),
            theme,
            platform: hudVariant ?? (hudSupported ? "notch" : "unsupported"),
            paused: !effectiveVisible,
            visible: effectiveVisible,
          },
        },
        "*",
      )
    node.onload = send
    send()
  }
  return (
    <>
      <iframe
        ref={frame}
        className="onboarding-demo-frame"
        data-ready={status === "ready"}
        title="Example demonstration"
        sandbox="allow-scripts"
        src="/onboarding-demo/index.html?bars=4&island=collapsed"
        onError={channel.fail}
      />
      {status === "error" && (
        <p className="onboarding-demo-error" role="status">
          The example could not load. You can continue.
        </p>
      )}
    </>
  )
}

export function OnboardingView({
  step,
  onStepChange,
  onComplete,
  busy = false,
  error,
  hudSupported = false,
  hudVariant,
  visible = true,
}: OnboardingViewProps) {
  if (step === "cloudUnavailable")
    return (
      <CloudOnboarding
        onBack={() => onStepChange("cloud")}
        onComplete={onComplete}
        busy={busy}
        error={error}
        visible={visible}
      />
    )
  const index = order.indexOf(step)
  const isTeaching = step !== "welcome"
  const slide = isTeaching ? slides[step] : null
  return (
    <main className="onboarding-shell">
      {step === "welcome" ? (
        <>
          <div className="onboarding-stage onboarding-welcome">
            <div className="onboarding-drag-region" data-tauri-drag-region />
            <img className="onboarding-logo" src={appIcon} alt="" />
            <div className="onboarding-agents">
              <p>Works with your coding agents</p>
              <div className="onboarding-agent-marks">
                {["claude-code", "codex", "cursor", "pi", "opencode"].map((agent) => (
                  <span key={agent}>{renderAgentIcon(agent, 24)}</span>
                ))}
              </div>
            </div>
          </div>
          <section className="onboarding-copy onboarding-copy-welcome">
            <h1 className="type-onboarding-title" tabIndex={-1} ref={(node) => node?.focus()}>
              Stop hitting your token limits
            </h1>
            <p className="type-onboarding-body">
              See where your coding agents spend tokens and what you can turn off, so you can
              work more effectively with less waste.
            </p>
            <small>Free. Open source. Runs locally.</small>
          </section>
        </>
      ) : (
        <>
          <div
            className={`onboarding-stage ${step === "limits" || step === "hud" ? "onboarding-stage-edge" : "onboarding-stage-inset"}`}
          >
            <div className="onboarding-drag-region" data-tauri-drag-region />
            <Demo
              key={step}
              step={step}
              hudSupported={hudSupported}
              {...(hudVariant ? { hudVariant } : {})}
              visible={visible}
            />
            {step === "cloud" && (
              <span className="onboarding-example-label">
                Example only · No sessions are transferred
              </span>
            )}
          </div>
          <section className="onboarding-copy">
            <h1
              key={step}
              className="type-onboarding-title"
              tabIndex={-1}
              ref={(node) => node?.focus()}
            >
              {slide?.title}
            </h1>
            <p className="type-onboarding-body">
              {step === "limits" && !hudSupported
                ? "Check your providers’ usage and reset times from the system tray, without interrupting your work."
                : slide?.body}
            </p>
            <small>{slide?.support}</small>
          </section>
        </>
      )}
      <footer className="onboarding-footer">
        <div>
          {step !== "welcome" && (
            <button
              className="onboarding-secondary"
              disabled={busy}
              onClick={() => onStepChange(order[index - 1]!)}
            >
              Back
            </button>
          )}
        </div>
        <div className="onboarding-actions">
          {error && <span role="alert">{error}</span>}
          {step === "cloud" ? (
            <button className="onboarding-secondary" disabled={busy} onClick={onComplete}>
              Use locally for now
            </button>
          ) : null}
          {
            <button
              className="onboarding-primary"
              disabled={busy}
              onClick={() =>
                onStepChange(step === "cloud" ? "cloudUnavailable" : order[index + 1]!)
              }
            >
              {step === "welcome"
                ? "Get started"
                : step === "cloud"
                  ? "Set up Cloud"
                  : "Continue"}
            </button>
          }
        </div>
      </footer>
    </main>
  )
}
