import { useState, useSyncExternalStore } from "react"
import { invoke } from "@tauri-apps/api/core"
import { CloudOnboarding } from "./CloudOnboarding"
import { createCloudDevPort, isCloudDevScenario, type CloudDevScenario } from "./cloudDevPort"
declare global {
  interface Window {
    __ANTIBURN_CLOUD_DEV__?: string
  }
}
let selection = { scenario: "walkthrough" as CloudDevScenario, revision: 0 }
if (isCloudDevScenario(window.__ANTIBURN_CLOUD_DEV__))
  selection.scenario = window.__ANTIBURN_CLOUD_DEV__
const listeners = new Set<() => void>()
function select(scenario: CloudDevScenario) {
  window.__ANTIBURN_CLOUD_DEV__ = scenario
  selection = { scenario, revision: selection.revision + 1 }
  listeners.forEach((listener) => listener())
}
function changed(event: Event) {
  const detail: unknown = (event as CustomEvent<unknown>).detail
  if (isCloudDevScenario(detail)) select(detail)
}
function subscribe(listener: () => void) {
  if (!listeners.size) window.addEventListener("cloud-dev:scenario", changed)
  listeners.add(listener)
  return () => {
    listeners.delete(listener)
    if (!listeners.size) window.removeEventListener("cloud-dev:scenario", changed)
  }
}
const snapshot = () => {
  const current = window.__ANTIBURN_CLOUD_DEV__
  if (isCloudDevScenario(current) && current !== selection.scenario)
    selection = { scenario: current, revision: selection.revision + 1 }
  return selection
}
export async function revealCloudDev() {
  await invoke("cloud_preview_window", { action: "ready" })
}
function Preview({ scenario }: { scenario: CloudDevScenario }) {
  const [port] = useState(() => createCloudDevPort(scenario))
  return (
    <CloudOnboarding
      port={port}
      analytics={false}
      holdIdentity={scenario === "identity"}
      onBack={() => select("walkthrough")}
      onComplete={() => void invoke("cloud_preview_window", { action: "close" })}
    />
  )
}
export function CloudDev() {
  const selected = useSyncExternalStore(subscribe, snapshot, snapshot)
  return (
    <div
      className="h-full"
      onMouseDown={(event) => {
        if (
          event.button === 0 &&
          (event.target as HTMLElement).closest("[data-tauri-drag-region]")
        )
          void invoke("cloud_preview_window", { action: "drag" })
      }}
    >
      <Preview key={selected.revision} scenario={selected.scenario} />
    </div>
  )
}
