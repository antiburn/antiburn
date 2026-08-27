import { Check } from "lucide-react"

import { Card } from "../../components/ui/Card"
import { Pane } from "../../components/ui/Pane"
import { PushButton } from "../../components/ui/PushButton"
import { RangeSlider } from "../../components/ui/RangeSlider"
import { Row } from "../../components/ui/Row"
import { SectionGroup } from "../../components/ui/SectionGroup"
import { SegmentedControl } from "../../components/ui/SegmentedControl"
import { ToggleRow } from "../../components/ui/ToggleRow"
import { cn } from "../../lib/cn"
import {
  postSampleNotification,
  postTestNotification,
  type DiskSpaceDisplay,
  type Milestones,
  type NudgePlacement,
  type SampleNotificationKind,
} from "../../lib/ipc"
import { isMacOS } from "../../lib/platform"
import type { AppSettingsController } from "./useAppSettings"

/**
 * Notifications.
 *
 * The pane lists every kind rather than describing a category: a reader
 * deciding whether to allow notifications is entitled to know precisely what
 * they are agreeing to be interrupted by. The shell is the only thing that
 * posts one (`src-tauri/src/notifications.rs`) and enforces the same gates
 * this pane presents.
 *
 * Delivery is antiburn's own notification window, so its presentation —
 * placement, how long it stays, whether it chimes — is decided here rather
 * than in the system settings. The test button exists because of that: the
 * system no longer previews these, so the pane must.
 *
 * The update and scan-failure kinds have no rows of their own — they are
 * named in the master switch's copy and governed by it (their per-kind
 * preferences persist for a hand-edited row, defaulting on). The milestone
 * rows carry a second gate that is not theirs: they fire only while
 * Settings → Usage is set to refresh, because a milestone needs readings that
 * keep moving and only that setting makes them move. The copy says so here
 * rather than leaving a reader to wonder why their selections are quiet.
 */

export type NotificationsPaneProps = AppSettingsController

const PLACEMENTS: readonly { value: NudgePlacement; label: string }[] = [
  { value: "menuBar", label: "Menu bar" },
  { value: "topRight", label: "Top right" },
]

const DISK_DISPLAYS: readonly { value: DiskSpaceDisplay; label: string }[] = [
  { value: "always", label: "Always" },
  { value: "whenLow", label: "When low" },
  { value: "never", label: "Never" },
]

/** The threshold presets. The store accepts 5–2000; these are the sensible
 *  stops, matching the tray's own "N GB" rendering. */
const DISK_THRESHOLDS: readonly { value: string; label: string }[] = [
  { value: "25", label: "25 GB" },
  { value: "50", label: "50 GB" },
  { value: "100", label: "100 GB" },
]

/** Every kind the shell can post, for the debug-only sample row. */
const SAMPLE_KINDS: readonly { value: SampleNotificationKind; label: string }[] = [
  { value: "updateAvailable", label: "Update" },
  { value: "scanFailure", label: "Scan failure" },
  { value: "diskSpaceLow", label: "Disk low" },
  { value: "usageMilestone", label: "Milestone" },
  { value: "menuBarHome", label: "Menu bar home" },
  { value: "test", label: "Test" },
]

const MILESTONE_OPTIONS = Array.from({ length: 20 }, (_, index) => (index + 1) * 5)

function MilestoneSelector({
  value,
  onChange,
  ariaLabel,
  disabled = false,
}: {
  value: Milestones
  onChange: (next: Milestones) => void
  ariaLabel: string
  disabled?: boolean
}) {
  const selected = new Set(value)
  const toggle = (threshold: number) => {
    const next = selected.has(threshold)
      ? value.filter((item) => item !== threshold)
      : [...value, threshold].sort((left, right) => left - right)
    onChange(next)
  }

  return (
    <div role="group" aria-label={ariaLabel} className="mt-2">
      <div className="mb-2 flex items-center gap-2">
        <PushButton
          onClick={() => onChange([...MILESTONE_OPTIONS])}
          disabled={disabled || value.length === MILESTONE_OPTIONS.length}
        >
          Select all
        </PushButton>
        <PushButton onClick={() => onChange([])} disabled={disabled || value.length === 0}>
          Clear all
        </PushButton>
      </div>
      <div className="grid grid-flow-col grid-rows-4 gap-1">
        {MILESTONE_OPTIONS.map((threshold) => {
          const checked = selected.has(threshold)
          return (
            <button
              key={threshold}
              type="button"
              role="checkbox"
              aria-checked={checked}
              disabled={disabled}
              onClick={() => toggle(threshold)}
              className={cn(
                "flex items-center gap-1.5 rounded-control px-1 py-1 transition-colors duration-[var(--duration-fast)] ease-out hover:bg-surface-hover disabled:opacity-50",
                checked ? "text-label" : "text-label-secondary",
              )}
            >
              <span
                aria-hidden="true"
                className={cn(
                  "flex size-3.5 shrink-0 items-center justify-center rounded-small border transition-colors duration-[var(--duration-fast)]",
                  checked
                    ? "border-accent-fill bg-accent-fill text-white"
                    : "border-separator bg-input-fill",
                )}
              >
                {checked && <Check size={12} strokeWidth={3} />}
              </span>
              <span className="type-footnote tabular-nums">{threshold}%</span>
            </button>
          )
        })}
      </div>
    </div>
  )
}

export function NotificationsPane({ settings, update }: NotificationsPaneProps) {
  const on = settings.notificationsEnabled
  const macOS = isMacOS()

  return (
    <Pane title="Notifications">
      <SectionGroup title="Allow">
        <Card>
          <ToggleRow
            label="Notify me"
            description="antiburn interrupts you for a newer version, a scan that could not finish, and the alerts below — nothing else. There is no digest, no progress notification, and no marketing."
            checked={on}
            onChange={(next) => void update({ notificationsEnabled: next })}
          />
          <ToggleRow
            label="Sound"
            description="A short chime, generated by antiburn itself. The test notification plays it; other notifications stay quiet."
            checked={settings.notificationSound}
            onChange={(next) => void update({ notificationSound: next })}
            dimmed={!on}
            disabled={!on}
          />
          <Row
            label="Auto-dismiss time"
            description="How long a notification stays before it fades. Hovering pauses the timer."
            trailing={
              <span className="type-body tabular-nums text-label-secondary">
                {settings.nudgeAutoDismissSecs}s
              </span>
            }
          >
            <RangeSlider
              className="mt-2 w-full"
              value={settings.nudgeAutoDismissSecs}
              min={3}
              max={30}
              ariaLabel="Seconds before a notification dismisses itself"
              ariaValueText={`${settings.nudgeAutoDismissSecs} seconds`}
              onChange={(secs) => void update({ nudgeAutoDismissSecs: secs })}
            />
          </Row>
          <Row
            label="Test notification"
            // Deliberately never dimmed: the test bypasses the master switch,
            // so a reader can see what they would be allowing before they
            // allow it.
            description="Show a sample notification, exactly as a real one would appear."
            trailing={
              <PushButton onClick={() => void postTestNotification()}>Show test</PushButton>
            }
          />
          {import.meta.env.DEV && (
            <Row
              label="Sample notifications"
              description="Debug builds only. Each button posts one kind with fixed figures, skipping every gate, so the copy can be checked on the real card."
            >
              <div className="mt-2 flex flex-wrap gap-2">
                {SAMPLE_KINDS.map(({ value, label }) => (
                  <PushButton key={value} onClick={() => void postSampleNotification(value)}>
                    {label}
                  </PushButton>
                ))}
              </div>
            </Row>
          )}
          {macOS && (
            <Row
              label="Position"
              description="Where notifications appear: hanging under the antiburn menu-bar icon, or at the screen's top-right corner."
              trailing={
                <SegmentedControl
                  options={PLACEMENTS}
                  value={settings.nudgePlacement}
                  onChange={(next) => void update({ nudgePlacement: next })}
                  ariaLabel="Notification position"
                />
              }
            />
          )}
        </Card>
      </SectionGroup>

      <SectionGroup title="Usage limits">
        <Card>
          <Row
            label="5-hour milestones"
            description="Choose which quota percentages notify. Each notification compares quota used with time elapsed in the five-hour window. These fire only while Settings → Usage is set to refresh."
            dimmed={!on}
          >
            <MilestoneSelector
              value={settings.milestones5h}
              onChange={(next) => void update({ milestones5h: next })}
              ariaLabel="Five-hour milestone thresholds"
              disabled={!on}
            />
          </Row>
          <Row
            label="Weekly milestones"
            description="Choose weekly quota percentages separately. Notifications re-arm when the weekly limit resets."
            dimmed={!on}
          >
            <MilestoneSelector
              value={settings.milestonesWeekly}
              onChange={(next) => void update({ milestonesWeekly: next })}
              ariaLabel="Weekly milestone thresholds"
              disabled={!on}
            />
          </Row>
        </Card>
      </SectionGroup>

      {macOS && (
        <SectionGroup title="Remaining disk space">
          <Card>
            <Row
              label="Show in menu bar"
              description="Free space on your startup disk, next to the antiburn icon. The number matches what Finder reports."
              trailing={
                <SegmentedControl
                  options={DISK_DISPLAYS}
                  value={settings.diskSpaceDisplay}
                  onChange={(next) => void update({ diskSpaceDisplay: next })}
                  ariaLabel="When to show free disk space in the menu bar"
                />
              }
            />
            <Row
              label="Low when below"
              description="The level that counts as running low."
              trailing={
                <SegmentedControl
                  options={DISK_THRESHOLDS}
                  value={String(settings.diskSpaceThresholdGb)}
                  onChange={(next) => void update({ diskSpaceThresholdGb: Number(next) })}
                  ariaLabel="Low disk space threshold"
                />
              }
            />
            <ToggleRow
              label="Notify when low"
              description="Once each time free space drops below the level, and again only after it recovers."
              checked={settings.notifyDiskSpaceLow}
              onChange={(next) => void update({ notifyDiskSpaceLow: next })}
              dimmed={!on}
              disabled={!on}
            />
          </Card>
        </SectionGroup>
      )}
    </Pane>
  )
}
