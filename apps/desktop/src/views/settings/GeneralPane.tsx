import { useCallback, useState, useSyncExternalStore } from "react"
import { confirm } from "@tauri-apps/plugin-dialog"

import { Card } from "../../components/ui/Card"
import { Pane } from "../../components/ui/Pane"
import { PushButton } from "../../components/ui/PushButton"
import { RangeSlider } from "../../components/ui/RangeSlider"
import { Row } from "../../components/ui/Row"
import { SectionGroup } from "../../components/ui/SectionGroup"
import { StatusText } from "../../components/ui/StatusText"
import { ToggleRow } from "../../components/ui/ToggleRow"
import {
  cancelScan,
  closeCurrentWindow,
  restartOnboarding,
  scanNow,
  type AppInfo,
  type ScanStatus,
} from "../../lib/ipc"
import { relativeTime } from "../../lib/presentation/relativeTime"
import { scanStatusStore } from "../../lib/scanStatusStore"
import type { AppSettingsController } from "./useAppSettings"

/** Narrowest and widest activity-list windows, mirroring the store's clamp. */
// Mirrors the shell's MIN/MAX_ACTIVITY_DAYS: the ceiling equals the store's
// display range only; indexed sessions outside it remain stored.
const MIN_DAYS = 1
const MAX_DAYS = 14

function dayLabel(days: number): string {
  return days === 1 ? "1 day" : `${days} days`
}

/** A byte count at a readable scale. Two significant places is enough for a
 *  settings row: the question being answered is "is this large?". */
export function byteLabel(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return "0 KB"
  const units = ["KB", "MB", "GB"]
  let value = bytes / 1024
  let unit = 0
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024
    unit += 1
  }
  return `${value < 10 ? value.toFixed(1) : Math.round(value)} ${units[unit]}`
}

/** What the historical-scan row says about the last (or current) pass. */
export function scanSummary(status: ScanStatus | null): string {
  if (status?.running) return "Scanning now…"
  if (status?.error) return "The last scan did not finish."
  if (status?.cancelled) return "The last scan was stopped before it finished."
  if (status?.finishedAt) return `Last scanned ${relativeTime(status.finishedAt)}.`
  return "Nothing has been scanned yet."
}

export interface GeneralPaneProps extends AppSettingsController {
  /** Absent until the shell answers; `null` outside the shell entirely. */
  info: AppInfo | null
}

/**
 * General preferences: how much history the popover shows, whether antiburn
 * keeps looking on its own, and how much it has accumulated.
 *
 * The monitoring toggle here and the pause control in the popover footer are
 * the same preference. That is deliberate: the footer is where a reader notices
 * background work happening, and Settings is where they go looking for the
 * switch that stops it.
 */
export function GeneralPane({ settings, update, info }: GeneralPaneProps) {
  const [restartingSetup, setRestartingSetup] = useState(false)
  const [restartFailed, setRestartFailed] = useState(false)
  const scanStatus = useSyncExternalStore(
    scanStatusStore.subscribe,
    scanStatusStore.getSnapshot,
  )

  const handleScan = useCallback(async () => {
    const status = await scanNow().catch(() => null)
    if (status) scanStatusStore.set(status)
  }, [])

  const handleCancel = useCallback(async () => {
    const status = await cancelScan().catch(() => null)
    if (status) scanStatusStore.set(status)
  }, [])

  async function handleRestartOnboarding() {
    setRestartFailed(false)
    try {
      const proceed = await confirm(
        "Setup opens at the Welcome step. Your indexed sessions and current settings stay on this machine. If you close setup before you finish, it returns the next time you open antiburn.",
        { title: "Run setup again?", kind: "warning", okLabel: "Run setup again" },
      )
      if (!proceed) return

      setRestartingSetup(true)
      await restartOnboarding()
      await closeCurrentWindow()
    } catch {
      setRestartFailed(true)
    } finally {
      setRestartingSetup(false)
    }
  }

  const running = scanStatus?.running ?? false

  return (
    <Pane title="General">
      {/* Monitoring leads: whether antiburn is looking at all is the pane's
          primary switch, and everything below describes what the looking
          produced. */}
      <SectionGroup title="Monitoring">
        <Card>
          <ToggleRow
            label="Keep looking for new sessions"
            description="antiburn re-reads your agents' session files while the popover is open. Turning this off stops the background pass — everything already indexed stays readable, and you can still scan on demand."
            checked={!settings.discoveryPaused}
            onChange={(next) => void update({ discoveryPaused: !next })}
          />
          <Row
            label="Historical scan"
            description={`Read every session file antiburn can find on this machine, from the start. ${scanSummary(
              scanStatus,
            )}`}
            trailing={
              running ? (
                <PushButton onClick={() => void handleCancel()}>Stop</PushButton>
              ) : (
                <PushButton onClick={() => void handleScan()}>Scan now</PushButton>
              )
            }
          />
        </Card>
      </SectionGroup>

      <SectionGroup title="Activity">
        <Card>
          <Row
            label="Show the last"
            description={`The popover lists sessions from the last ${dayLabel(
              settings.activityWindowDays,
            )}. This changes the list, not storage; indexed sessions outside the window remain on this machine.`}
            trailing={
              <span className="type-body tabular-nums text-label-secondary">
                {dayLabel(settings.activityWindowDays)}
              </span>
            }
          >
            <RangeSlider
              className="mt-2 w-full"
              value={settings.activityWindowDays}
              min={MIN_DAYS}
              max={MAX_DAYS}
              ariaLabel="Days of activity to show"
              ariaValueText={dayLabel(settings.activityWindowDays)}
              onChange={(days) => void update({ activityWindowDays: days })}
            />
          </Row>
        </Card>
      </SectionGroup>

      <SectionGroup title="Local storage">
        <Card>
          <Row
            label="Indexed sessions"
            description="What antiburn currently has on this machine. Indexed sessions do not expire based on age; Settings → Privacy can clear them. Your agents' own files are never touched."
            trailing={
              <span className="type-body tabular-nums text-label-secondary">
                {info ? `${info.indexedSessions} · ${byteLabel(info.databaseBytes)}` : "—"}
              </span>
            }
          />
        </Card>
      </SectionGroup>

      <SectionGroup title="Startup">
        <Card>
          <ToggleRow
            label="Launch antiburn on startup"
            description="Starts automatically in the menu bar."
            checked={settings.launchAtLogin}
            onChange={(next) => void update({ launchAtLogin: next })}
          />
        </Card>
      </SectionGroup>

      <SectionGroup title="Setup">
        <Card>
          <Row
            label="Run setup again"
            description="Return to the Welcome step and review the setup choices. Indexed sessions, scan folders, repository choices, and current preferences stay on this machine."
            trailing={
              <PushButton
                onClick={() => void handleRestartOnboarding()}
                disabled={restartingSetup}
              >
                {restartingSetup ? "Opening…" : "Run setup again…"}
              </PushButton>
            }
          >
            <div role="status" aria-live="polite" aria-atomic="true">
              {restartFailed && (
                <StatusText tone="secondary" className="mt-1.5">
                  Setup could not open. Try again or restart antiburn.
                </StatusText>
              )}
            </div>
          </Row>
        </Card>
      </SectionGroup>
    </Pane>
  )
}
