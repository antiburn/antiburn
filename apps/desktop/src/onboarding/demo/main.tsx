import { HudDetailView } from "../../views/overlay/HudDetailView"
import { UsageView } from "../../views/popover/UsageView"
import { PopoverView } from "../../views/PopoverView"
import trayIcon from "../../../src-tauri/icons/tray.png"
import { localSessionKey } from "../../lib/presentation/localIdentity"
import { useSyncExternalStore } from "react"
import { createRoot } from "react-dom/client"
import { flushSync } from "react-dom"
import { SessionList } from "../../components/session/SessionList"
import { SessionPane } from "../../views/popover/SessionPane"
import { subjectForEntry } from "../../views/main-window/MainActivitySession"
import { renderAgentIcon } from "../../lib/agentIcon"
import { OverlayWindow } from "../../views/OverlayWindow"
import { NudgeView } from "../../views/NudgeView"
import { BurnChecksView } from "../../views/main-window/BurnChecksView"
import { BurnChecksController } from "../../views/main-window/burn-checks/BurnChecksController"
import { emitFixtureEvent } from "./adapters/event"
import { fixtureIsland } from "./adapters/hud"
import * as f from "./prototypeFixtures"
import "../../styles.css"
import "./runtime.css"
import "./theme-fixes.css"
function Pointer() {
  return (
    <svg width="22" height="28" viewBox="0 0 22 28" fill="none" aria-hidden="true">
      <path
        d="M2 1.5v22l5.6-5.3 4.2 8.3 3.8-1.9-4.2-8.1 7.5-.5L2 1.5Z"
        fill="#111"
        stroke="white"
        strokeWidth="1.5"
        strokeLinejoin="round"
      />
    </svg>
  )
}
function MenuBar({ interactive = false }: { interactive?: boolean }) {
  return (
    <div className="desktop-edge">
      <div className="menu-app" aria-hidden="true" />
      <div className="menu-status">
        <button
          className="tray-anchor"
          aria-label="Open Antiburn usage example"
          onClick={() => interactive && go(3)}
          tabIndex={interactive ? 0 : -1}
        >
          <span
            className="tray-template"
            style={{
              maskImage: `url(${trayIcon})`,
              WebkitMaskImage: `url(${trayIcon})`,
            }}
          />
        </button>
        <svg width="17" height="14" viewBox="0 0 20 16" aria-hidden="true">
          <path
            d="M2 5q8-7 16 0M5 8q5-4 10 0M8 11q2-2 4 0"
            fill="none"
            stroke="currentColor"
            strokeWidth="1.7"
            strokeLinecap="round"
          />
          <circle cx="10" cy="14" r="1" fill="currentColor" />
        </svg>
        <svg width="23" height="13" viewBox="0 0 25 14" aria-hidden="true">
          <rect x="1" y="1" width="20" height="12" rx="3" fill="none" stroke="currentColor" />
          <rect x="3" y="3" width="15" height="8" rx="1" fill="currentColor" />
          <path d="M23 5v4" stroke="currentColor" strokeWidth="2" />
        </svg>
        <svg width="15" height="15" viewBox="0 0 18 18" aria-hidden="true">
          <circle cx="7" cy="7" r="4.5" fill="none" stroke="currentColor" strokeWidth="1.5" />
          <path d="m11 11 4 4" stroke="currentColor" strokeWidth="1.5" />
        </svg>
        <svg width="17" height="16" viewBox="0 0 20 18" aria-hidden="true">
          <rect x="1" y="1" width="18" height="6" rx="3" fill="none" stroke="currentColor" />
          <circle cx="5" cy="4" r="2" fill="currentColor" />
          <rect x="1" y="11" width="18" height="6" rx="3" fill="none" stroke="currentColor" />
          <circle cx="15" cy="14" r="2" fill="currentColor" />
        </svg>
        <span>Tue 9:41</span>
      </div>
    </div>
  )
}
const fixtureNow = Date.now()
const entries = f.sessions.slice(0, 3).map((s, index) => ({
  ...s,
  remoteHostId: index === 1 ? "prototype-cloud-codex" : s.remoteHostId,
  surface: "cli" as const,
  title: s.title ?? undefined,
  cost: s.cost ? { ...s.cost, figureLabel: "Projected cost" } : null,
  isActive: true,
}))
let snapshot = {
  theme: new URLSearchParams(window.location.search).get("theme") === "dark" ? "dark" : "light",
  destination: "introduction",
  slide: 0,
  step: 0,
  elapsed: 0,
  transferProgress: 0,
  platform: "notch",
  paused: false,
  visible: false,
  zoom: 1,
  sessionIndex: 0,
  limitsExpanded: true,
  hoveredProvider: null as string | null,
}
const sessionAnalyses: ReturnType<typeof f.analysis>[] = []
function sessionAnalysis(index: number) {
  return (sessionAnalyses[index] ??= f.analysis(entries[index]!.sessionId!))
}
const hygiene = new Map(
  entries.map((s) => [
    localSessionKey(s.agent, s.sessionId!, s.wslDistro, s.remoteHostId),
    f.hygiene(s.sessionId!),
  ]),
)
const listeners = new Set<() => void>()
const subscribe = (fn: () => void) => {
  listeners.add(fn)
  return () => listeners.delete(fn)
}
let checks: BurnChecksController | undefined
function checksController() {
  return (checks ??= new BurnChecksController())
}
let revision = 20
let lastIslandState: string | undefined
let readySent = false
let checksGeometry: { target: Element; viewport: HTMLElement; destination: number } | undefined
window.addEventListener("resize", () => {
  checksGeometry = undefined
})
const notify = (type: string, extra = {}) =>
  parent.postMessage({ source: "antiburn-onboarding-demo", type, ...extra }, "*")
function island(open: boolean) {
  const key = `${snapshot.platform}:${open}`
  if (lastIslandState === key) return
  lastIslandState = key
  document.querySelector(".hud-scene")?.classList.toggle("hud-expanded", open)
  const params = new URLSearchParams(
    "island=" + (snapshot.platform === "floating" ? "off" : open ? "expanded" : "collapsed"),
  )
  emitFixtureEvent("hud-island:state", {
    ...fixtureIsland(params),
    revision: ++revision,
  })
}
function go(step: number) {
  notify("step", { step })
}
function smoothProgress(time: number, start: number, duration: number) {
  const t = Math.max(0, Math.min(1, (time - start) / duration))
  return t * t * (3 - 2 * t)
}
function Preview() {
  const s = useSyncExternalStore(subscribe, () => snapshot)
  const approach = smoothProgress(s.elapsed, 700, 450)
  const reveal = smoothProgress(s.elapsed, 1250, 300)
  if (!s.visible) return null
  if (s.slide === 0)
    return (
      <div className={"sessions-scene " + (s.step >= 2 ? "detail-open" : "detail-closed")}>
        <span
          className={"session-pointer session-pointer-" + s.step}
          style={{
            opacity: smoothProgress(s.elapsed, 250, 200),
            transform: `translate(${630 - 210 * approach - 236 * reveal}px, ${245 - 153 * approach}px)`,
            transition: "none",
          }}
          aria-hidden="true"
        >
          <Pointer />
        </span>
        <section
          className="sessions-list"
          style={{
            transform: `translateX(${236 * (1 - reveal)}px)`,
            transition: "none",
          }}
        >
          <SessionList
            entries={entries}
            remoteHosts={[
              {
                id: "prototype-cloud-codex",
                sshAlias: "Codex Cloud",
                displayName: "Codex Cloud",
                automaticSyncEnabled: false,
                status: "idle",
                lastSuccessfulSyncEpoch: null,
                cachedSessionCount: 1,
                lastError: null,
              },
            ]}
            days={7}
            now={new Date()}
            renderAgentIcon={renderAgentIcon}
            active={!s.paused}
            hygieneBySession={hygiene}
            onOpenSession={(entry) => {
              snapshot = {
                ...snapshot,
                sessionIndex: entries.findIndex((item) => item.sessionId === entry.sessionId),
              }
              listeners.forEach((listener) => listener())
              go(2)
            }}
            openOnClick
          />
        </section>
        <section className="session-detail" inert={s.step < 2} aria-hidden={s.step < 2}>
          <SessionPane
            subject={subjectForEntry(entries[s.sessionIndex]!)}
            payload={sessionAnalysis(s.sessionIndex)}
            loading={false}
            refreshing={false}
            error={false}
            onOpenSession={() => {}}
            onDeleted={() => {}}
            embedded
            active
          />
        </section>
      </div>
    )
  if (s.slide === 1) {
    const shownProvider =
      s.hoveredProvider ?? (s.elapsed >= 2300 && s.elapsed < 4500 ? "anthropic" : null)
    const live = {
      ...f.liveUsage,
      providers: f.liveUsage.providers.slice(0, 2).map((p) => ({
        ...p,
        sourceLabel: "",
        windows: p.windows.map((w, i) => ({
          ...w,
          usedPercent: i ? 44 : 72,
          resetsAt: new Date(fixtureNow + (i ? 86400000 : 3600000)).toISOString(),
          forecast: {
            ...w.forecast,
            paceRatio: 0.9,
            consumptionRate: i ? 1.2 : 18,
            runwayAt: new Date(fixtureNow + (i ? 168000000 : 5600000)).toISOString(),
          },
        })),
      })),
    }
    return (
      <div className="limits-scene">
        <div
          style={{
            transform: `translateY(${matchMedia("(prefers-reduced-motion: reduce)").matches ? 0 : -32 * (1 - smoothProgress(s.elapsed, 0, 300))}px)`,
          }}
        >
          <MenuBar interactive />
        </div>
        <div className={"popover-crop " + (s.step >= 2 ? "shown" : "")} inert={s.step < 2}>
          <div className="real-popover">
            <PopoverView />
          </div>
        </div>
        {shownProvider && s.step >= 2 && (
          <aside
            className="usage-companion"
            aria-label="Provider usage preview"
            onPointerEnter={() => clearTimeout(companionCloseTimer)}
            onPointerLeave={() => scheduleCompanionClose()}
          >
            <div data-popover-peek-usage className="pt-2 h-full">
              <UsageView
                summary={{
                  ...f.usage,
                  providers: f.usage.providers.filter((p) => p.provider === shownProvider),
                }}
                live={{
                  ...live,
                  providers: live.providers.filter((p) => p.provider === shownProvider),
                }}
                now={fixtureNow}
              />
            </div>
          </aside>
        )}
        <div
          className={"menu-pointer menu-pointer-" + s.step}
          style={{
            opacity:
              smoothProgress(s.elapsed, 350, 200) * (1 - smoothProgress(s.elapsed, 5100, 300)),
          }}
          aria-hidden="true"
        >
          <Pointer />
        </div>
      </div>
    )
  }
  if (s.slide === 2)
    return (
      <div className={"hud-scene " + s.platform}>
        {s.platform !== "floating" && <MenuBar />}
        <div className="hud-mount">
          {s.platform === "unsupported" ? (
            <p className="no-hud">Check limits in the app or system tray.</p>
          ) : (
            <OverlayWindow key={s.platform} />
          )}
        </div>
        {s.platform === "floating" && (
          <div className="floating-detail">
            <HudDetailView />
          </div>
        )}
        {s.platform !== "unsupported" && (
          <div
            className="hud-hover-target"
            aria-hidden="true"
            onMouseEnter={() => island(true)}
            onMouseLeave={() => island(false)}
          />
        )}
        <div
          className={"demo-pointer pointer-" + s.step}
          style={{
            opacity:
              smoothProgress(s.elapsed, 250, 200) * (1 - smoothProgress(s.elapsed, 3250, 250)),
          }}
          aria-hidden="true"
        >
          <Pointer />
        </div>
        <div
          aria-label="Example notification"
          className={"notification-mount " + (s.step >= 4 ? "notification-visible" : "")}
        >
          <NudgeView />
        </div>
      </div>
    )
  if (s.slide === 3)
    return (
      <div className={"checks-scene step-" + s.step} inert>
        <BurnChecksView
          active
          session={checksController()}
          focusedCheck={s.step >= 2 ? "modelOverthinking" : "unusedSkills"}
          focusRevision={s.step >= 2 ? 2 : 1}
        />
        <span
          className="click-cue check-cue"
          style={{ opacity: smoothProgress(s.elapsed, 250, 200) }}
          aria-hidden="true"
        >
          <Pointer />
        </span>
      </div>
    )
  if (s.slide === 4)
    return (
      <div className={"devices-scene devices-step-" + s.step}>
        <section className="device-preview device-origin">
          <header>
            MacBook Pro <span>This device</span>
          </header>
          <div className="device-screen">
            <SessionList
              entries={[entries[0]!]}
              days={7}
              now={new Date()}
              renderAgentIcon={renderAgentIcon}
              hygieneBySession={hygiene}
              active={false}
              onOpenSession={() => {}}
            />
          </div>
        </section>
        <div
          className="device-sync"
          aria-hidden="true"
          style={{
            opacity: s.step >= 3 ? Math.min(1, s.transferProgress * 4) : 0,
            transform: `translateX(${-4 * (1 - Math.min(1, s.transferProgress * 4))}px)`,
          }}
        >
          <svg width="32" height="24" viewBox="0 0 32 24" fill="none">
            <path
              d="M3 12h24m-7-7 7 7-7 7"
              stroke="currentColor"
              strokeWidth="1.5"
              strokeLinecap="round"
              strokeLinejoin="round"
            />
            <path
              className="transfer-highlight"
              d="M3 12h24m-7-7 7 7-7 7"
              pathLength="40"
              stroke="currentColor"
              strokeWidth="2"
              strokeLinecap="round"
              strokeLinejoin="round"
              strokeDasharray="9 50"
              strokeDashoffset={12 - s.transferProgress * 62}
              opacity={s.step === 3 ? Math.sin(Math.PI * s.transferProgress) : 0}
            />
          </svg>
        </div>
        <section
          className="device-preview device-destination"
          aria-hidden={s.step < 2}
          inert={s.step < 2}
        >
          <header>
            Mac mini <span>Another device</span>
          </header>
          <div className="device-screen">
            <div
              className={"synced-session " + (s.step >= 4 ? "is-visible" : "")}
              aria-hidden={s.step < 4}
              inert={s.step < 4}
            >
              <SessionList
                entries={[entries[0]!]}
                days={7}
                now={new Date()}
                renderAgentIcon={renderAgentIcon}
                hygieneBySession={hygiene}
                active={false}
                onOpenSession={() => {}}
              />
            </div>
          </div>
        </section>
      </div>
    )
  return null
}
document.addEventListener(
  "click",
  (event) => {
    if ((event.target as Element | null)?.closest("a")) event.preventDefault()
  },
  true,
)
document.addEventListener("submit", (event) => event.preventDefault(), true)

let costScrollInterrupted = false
document.addEventListener(
  "wheel",
  (event) => {
    if ((event.target as Element).closest(".session-detail")) costScrollInterrupted = true
  },
  { passive: true },
)
document.addEventListener("click", (event) => {
  const button = (event.target as Element).closest("button")
  if (event.isTrusted && snapshot.slide === 0 && button?.textContent?.trim() === "Cost") go(4)
})
const root = createRoot(document.getElementById("root")!)
root.render(<Preview />)
window.addEventListener("message", (e) => {
  if (e.source !== parent || e.data?.source !== "antiburn-onboarding") return
  clearTimeout(companionCloseTimer)
  const state = e.data.state
  if (
    !state ||
    !Number.isInteger(state.slide) ||
    state.slide < 0 ||
    state.slide > 4 ||
    !Number.isInteger(state.step) ||
    state.step < 0 ||
    state.step > 5 ||
    typeof state.elapsed !== "number" ||
    !Number.isFinite(state.elapsed) ||
    state.elapsed < 0 ||
    !["light", "dark"].includes(state.theme) ||
    !["notch", "floating", "unsupported"].includes(state.platform) ||
    typeof state.paused !== "boolean" ||
    typeof state.visible !== "boolean" ||
    typeof state.transferProgress !== "number" ||
    !Number.isFinite(state.transferProgress)
  )
    return
  const old = snapshot
  snapshot = {
    ...snapshot,
    slide: state.slide,
    step: state.step,
    elapsed: state.elapsed,
    theme: state.theme,
    platform: state.platform,
    paused: state.paused,
    visible: state.visible,
    transferProgress: Math.max(0, Math.min(1, state.transferProgress)),
  }
  if (
    snapshot.step < 4 ||
    old.step !== snapshot.step ||
    old.sessionIndex !== snapshot.sessionIndex
  )
    costScrollInterrupted = false
  document.documentElement.dataset.storyboardSlide = String(snapshot.slide)
  document.documentElement.style.zoom = String(snapshot.zoom)
  document.documentElement.dataset.theme = snapshot.theme === "dark" ? "dark" : "light"
  document.documentElement.dataset.platform =
    snapshot.platform === "unsupported" ? "windows" : "macos"
  document.body.dataset.paused = String(snapshot.paused)
  if (snapshot.slide === 0 && snapshot.step === 0) snapshot = { ...snapshot, sessionIndex: 0 }
  const renderChanged = Object.keys(snapshot).some((key) => {
    if (snapshot.slide === 3 && (key === "elapsed" || key === "transferProgress")) return false
    return snapshot[key as keyof typeof snapshot] !== old[key as keyof typeof old]
  })
  if (renderChanged) flushSync(() => listeners.forEach((fn) => fn()))
  if (snapshot.visible && !readySent) {
    readySent = true
    notify("ready")
  }
  if (!snapshot.visible) {
    lastIslandState = undefined
    checksGeometry = undefined
    return
  }
  if (
    snapshot.slide === 0 &&
    snapshot.step >= 2 &&
    (old.step !== snapshot.step || old.sessionIndex !== snapshot.sessionIndex || !old.visible)
  ) {
    const label = snapshot.step >= 4 ? "Cost" : "Context"
    requestAnimationFrame(() => {
      if (!snapshot.visible) return
      const button = [...document.querySelectorAll("button")].find(
        (el) => el.textContent?.trim() === label,
      )
      button?.click()
    })
  }
  if (snapshot.slide === 2) {
    if (old.slide !== 2 || old.platform !== snapshot.platform)
      setTimeout(() => {
        if (snapshot.visible) island(snapshot.step === 2)
      }, 100)
    else island(snapshot.step === 2)
  }
  requestAnimationFrame(() => {
    if (!snapshot.visible) return
    if (snapshot.slide === 0) {
      const panel = document.querySelector<HTMLElement>(".session-detail [role=tabpanel]")
      if (panel && snapshot.slide === 0 && !costScrollInterrupted) {
        const progress = Math.max(0, Math.min(1, (snapshot.elapsed - 5050) / 1600))
        const eased = 1 - Math.pow(1 - progress, 3)
        panel.scrollTop =
          snapshot.step >= 4
            ? Math.min(140, panel.scrollHeight - panel.clientHeight) * eased
            : 0
      }
      const sessionPointer = document.querySelector<HTMLElement>(".session-pointer")
      const scene = document.querySelector<HTMLElement>(".sessions-scene")
      const cost = [
        ...document.querySelectorAll<HTMLButtonElement>(".session-detail button"),
      ].find((el) => el.textContent?.trim() === "Cost")
      if (sessionPointer && scene && cost && snapshot.elapsed >= 3550) {
        const rect = cost.getBoundingClientRect(),
          origin = scene.getBoundingClientRect()
        const travel = smoothProgress(snapshot.elapsed, 3550, 400)
        sessionPointer.style.transform = `translate(${184 + (rect.left + rect.width / 2 - origin.left - 184) * travel}px, ${92 + (rect.top + rect.height / 2 - origin.top - 92) * travel}px)`
        sessionPointer.style.opacity = String(1 - smoothProgress(snapshot.elapsed, 4250, 250))
      } else if (sessionPointer)
        sessionPointer.style.opacity = String(smoothProgress(snapshot.elapsed, 250, 200))
    }
    if (snapshot.slide === 1) {
      const menuPointer = document.querySelector<HTMLElement>(".menu-pointer")
      const limitsScene = document.querySelector<HTMLElement>(".limits-scene")
      const dial = document.querySelector<HTMLElement>(
        '.real-popover [role="img"][tabindex="0"]',
      )
      if (menuPointer && limitsScene) {
        const t = snapshot.elapsed
        const approach = smoothProgress(t, 600, 400)
        let x = 161 * approach,
          y = -215 * approach
        if (dial && t >= 1600) {
          const bounds = dial.getBoundingClientRect(),
            origin = limitsScene.getBoundingClientRect()
          const dx = bounds.left + 16 - origin.left - 480
          const dy = bounds.top + bounds.height / 2 - origin.top - 230
          const hover = smoothProgress(t, 1600, 700)
          x = 161 + (dx - 161) * hover
          y = -215 + (dy + 215) * hover
          const leave = smoothProgress(t, 4500, 600)
          x += -110 * leave
          y += 100 * leave
        }
        menuPointer.style.transition = "none"
        menuPointer.style.transform = `translate(${x}px, ${y}px)`
      }
    }
    if (snapshot.slide !== 0 && snapshot.slide !== 3) return
    const cue = document.querySelector<HTMLElement>(".click-cue")
    if (cue) {
      let target: Element | undefined | null
      if (snapshot.slide === 0 && snapshot.step === 1)
        target = document.querySelector(".session-card")
      if (snapshot.slide === 0 && snapshot.step === 3)
        target = [...document.querySelectorAll("button")].find(
          (el) => el.textContent?.trim() === "Cost",
        )
      if (snapshot.slide === 3) {
        if (!checksGeometry?.target.isConnected) checksGeometry = undefined
        target =
          checksGeometry?.target ??
          [...document.querySelectorAll(".checks-scene span")].find(
            (el) => el.textContent?.trim() === "Model overthinking",
          )
      }
      if (target) {
        if (snapshot.slide === 3) {
          const viewport =
            checksGeometry?.viewport ??
            document.querySelector<HTMLElement>(".burn-checks-collection-scroll")
          if (viewport && (snapshot.elapsed <= 1300 || old.elapsed < 1300 || renderChanged)) {
            if (!checksGeometry) {
              const bounds = viewport.getBoundingClientRect()
              const row = target.closest("button") ?? target
              const rowBounds = row.getBoundingClientRect()
              const destination = Math.max(
                0,
                Math.min(
                  viewport.scrollHeight - viewport.clientHeight,
                  viewport.scrollTop +
                    rowBounds.top -
                    bounds.top -
                    (viewport.clientHeight - rowBounds.height) / 2,
                ),
              )
              checksGeometry = { target, viewport, destination }
            }
            const progress =
              matchMedia("(prefers-reduced-motion: reduce)").matches && snapshot.step >= 2
                ? 1
                : smoothProgress(snapshot.elapsed, 650, 650)
            viewport.scrollTop = checksGeometry.destination * progress
          }
        }
        const rect = target.getBoundingClientRect()
        const sceneRect = cue.parentElement!.getBoundingClientRect()
        const travel = smoothProgress(
          snapshot.elapsed,
          snapshot.slide === 3 ? 1400 : 600,
          snapshot.slide === 3 ? 500 : 400,
        )
        cue.style.left = "0"
        cue.style.top = "0"
        cue.style.transform = `translate(${650 + (rect.left + rect.width / 2 - sceneRect.left - 650) * travel}px, ${270 + (rect.top + rect.height / 2 - sceneRect.top - 270) * travel}px)`
        cue.style.opacity = String(
          smoothProgress(snapshot.elapsed, 250, 200) *
            (1 - smoothProgress(snapshot.elapsed, snapshot.slide === 3 ? 2200 : 1300, 200)),
        )
        cue.style.right = "auto"
        cue.style.bottom = "auto"
      }
    }
  })
})
window.addEventListener("error", (e) => notify("error", { message: e.message }))

function disableCheckActions() {
  document.querySelectorAll<HTMLButtonElement>(".session-detail button").forEach((button) => {
    if (button.textContent?.trim() === "Tools") button.hidden = true
  })
  document.querySelectorAll<HTMLButtonElement>(".checks-scene button").forEach((button) => {
    if (
      /^(Snooze|Fix|Copy fix prompt)$/.test(button.textContent?.trim() ?? "") &&
      !button.disabled
    ) {
      button.disabled = true
      button.title = "Unavailable in this onboarding preview"
    }
  })
}
new MutationObserver(disableCheckActions).observe(document.getElementById("root")!, {
  childList: true,
  subtree: true,
})

let companionCloseTimer: ReturnType<typeof setTimeout>
function scheduleCompanionClose() {
  clearTimeout(companionCloseTimer)
  companionCloseTimer = setTimeout(() => {
    if (!snapshot.visible) return
    if (document.querySelector(".usage-companion:hover")) return
    snapshot = { ...snapshot, hoveredProvider: null }
    listeners.forEach((fn) => fn())
  }, 250)
}
window.addEventListener("storyboard-provider", (event) => {
  const provider = (event as CustomEvent<string | null>).detail
  clearTimeout(companionCloseTimer)
  if (!provider) {
    scheduleCompanionClose()
    return
  }
  if (snapshot.hoveredProvider === provider) return
  snapshot = { ...snapshot, hoveredProvider: provider }
  listeners.forEach((fn) => fn())
})

function removePreviewTitles() {
  document.querySelectorAll<HTMLElement>("[title]").forEach((element) => {
    element.removeAttribute("title")
  })
}
new MutationObserver(removePreviewTitles).observe(document.body, {
  childList: true,
  subtree: true,
  attributes: true,
  attributeFilter: ["title"],
})
removePreviewTitles()
