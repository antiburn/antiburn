//! Watches free space on the startup volume, shows it beside the tray glyph,
//! and notifies once when it drops under the reader's threshold.
//!
//! Two outputs, one reading, deliberately independent controls:
//!
//! - **The number** ([`crate::tray_title`]) — ambient, always current, shown
//!   always / only when low / never per [`DiskSpaceDisplay`].
//! - **The notification** — a one-shot at the moment free space crosses down
//!   through the threshold. Edge-triggered, not level-triggered: a disk that
//!   stays full is not news, and a poll loop that re-notified every tick would
//!   be intolerable within the hour.
//!
//! macOS only for now. On other platforms [`free_bytes`] returns `None` and both
//! outputs stay silent.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tauri::{AppHandle, Manager, Runtime};

use crate::store::{DiskSpaceDisplay, Store};

/// How often to re-read free space. Disk usage moves slowly, and the reading is
/// a single volume stat, so this is about keeping the menu-bar number honest
/// rather than catching anything in the act.
const DISK_POLL: Duration = Duration::from_secs(60);

/// Delay before the first reading, to stay out of the way of launch. Shorter
/// than the update scheduler's ([`crate::updates::STARTUP_DELAY`]) — this makes
/// no network call, and the number looking "missing" for half a minute after
/// login would read as a bug.
const STARTUP_DELAY: Duration = Duration::from_secs(10);

/// How far free space must climb back above the threshold before the notification
/// re-arms, in GB.
///
/// Without it, a machine hovering at the threshold would re-notify every time
/// normal churn (a build, a browser cache) nudged it back and forth across the
/// line. Recovering this much is a real cleanup, and a real cleanup is the only
/// thing that should earn a second warning.
const REARM_HYSTERESIS_GB: u64 = 5;

/// Bytes per GB. **Binary** gibibytes, and truncating: 49.9 GB reads as 49, so
/// the number never claims space the reader does not have. Finder's "GB" is
/// decimal, so this reads a little lower than Finder for the same disk — the
/// threshold in Settings is in these same units.
const BYTES_PER_GB: u64 = 1_073_741_824;

/// Where the moment of the last low-disk notification is remembered across
/// launches.
///
/// An internal row rather than a preference: it is state, not a choice, so it
/// is namespaced under `internal:` and never surfaces in Settings. See
/// [`crate::store::Store::internal_value`].
const LAST_FIRED_KEY: &str = "internal:diskSpaceLowFiredMs";

/// Last reading, in GB, or [`u64::MAX`] for "not read yet".
///
/// Lets a preference change repaint the number immediately (see
/// [`refresh_title`]) instead of leaving the reader staring at the old state
/// until the next poll — a settings toggle that takes up to a minute to visibly
/// do anything reads as broken.
static LAST_FREE_GB: AtomicU64 = AtomicU64::new(u64::MAX);

/// Start the background poll. Idempotent per app launch — called once from
/// setup. The returned handle is aborted on exit, like every other scheduler.
pub fn spawn_disk_monitor(app: AppHandle) -> tauri::async_runtime::JoinHandle<()> {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(STARTUP_DELAY).await;
        let mut interval = tokio::time::interval(DISK_POLL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        // Seeded from the persisted stamp so relaunching on an already-full
        // disk is not treated as a fresh crossing. The in-memory trigger resets
        // every launch; the stored row is what remembers.
        let mut trigger = LowDiskTrigger {
            last_nudge_ms: last_fired_ms(&app),
        };

        loop {
            interval.tick().await;
            let Some(bytes) = free_bytes() else {
                continue;
            };
            let free_gb = bytes / BYTES_PER_GB;
            LAST_FREE_GB.store(free_gb, Ordering::Relaxed);
            refresh_title(&app);

            // Read fresh every tick: the threshold is a live preference, and a
            // cached copy would leave the reader's change waiting for a
            // relaunch. An unreadable store is not a reading, so skip the tick
            // rather than deciding against a default.
            let Ok(settings) = app.state::<Store>().settings() else {
                continue;
            };
            let threshold_gb = settings.disk_space_threshold_gb;
            // A shown or deliberately-consumed edge is marked. A suppressed one
            // is left pending and retried on the next poll.
            if trigger.should_nudge(free_gb, threshold_gb)
                && nudge_low_disk(&app, free_gb, threshold_gb)
            {
                let now_ms = now_ms();
                trigger.mark_fired(now_ms);
                if let Some(store) = app.try_state::<Store>() {
                    store.set_internal_value(LAST_FIRED_KEY, &now_ms.to_string());
                }
            }
        }
    })
}

/// Re-apply the menu-bar number from the last reading and the current settings.
///
/// Called on every poll, and again from `set_settings` so toggling the display
/// mode takes effect on the spot.
pub fn refresh_title<R: Runtime>(app: &AppHandle<R>) {
    let free_gb = LAST_FREE_GB.load(Ordering::Relaxed);
    if free_gb == u64::MAX {
        return; // nothing read yet — leave the tray as it is
    }

    let title = app
        .try_state::<Store>()
        .and_then(|store| store.settings().ok())
        .and_then(|settings| {
            title_for(
                settings.disk_space_display,
                free_gb,
                settings.disk_space_threshold_gb,
            )
        });

    if let Some(tray) = app.tray_by_id("antiburn") {
        crate::tray_title::set_tray_title(&tray, title);
    }
}

/// The text to draw beside the glyph, or `None` to draw nothing.
fn title_for(display: DiskSpaceDisplay, free_gb: u64, threshold_gb: u32) -> Option<String> {
    let low = is_low(free_gb, threshold_gb);
    match display {
        DiskSpaceDisplay::Always => Some(format_free(free_gb)),
        DiskSpaceDisplay::WhenLow if low => Some(format_free(free_gb)),
        DiskSpaceDisplay::WhenLow | DiskSpaceDisplay::Never => None,
    }
}

/// The menu-bar label: a whole number of GB and the unit, nothing else. The
/// menu bar is not the place for a decimal point.
fn format_free(free_gb: u64) -> String {
    format!("{free_gb} GB")
}

/// Strictly under the threshold: at exactly 50 GB the reader has 50 GB, which is
/// what they said was enough.
fn is_low(free_gb: u64, threshold_gb: u32) -> bool {
    free_gb < u64::from(threshold_gb)
}

/// Wall-clock milliseconds since the unix epoch.
///
/// A clock jump only ever moves the stored stamp, which is never compared
/// against — see [`LowDiskTrigger::mark_fired`] — so a system with a wrong clock
/// still gets exactly one warning per episode.
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| u64::try_from(since.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

/// The persisted stamp of the last low-disk notification, if there is one.
///
/// Absence — never written, an unreadable store, a value that is not a
/// number — all mean "act as if new", which costs at most one extra warning
/// after a relaunch onto an already-full disk.
fn last_fired_ms(app: &AppHandle) -> Option<u64> {
    app.try_state::<Store>()?
        .internal_value(LAST_FIRED_KEY)?
        .trim()
        .parse()
        .ok()
}

/// Edge detection for the low-disk notification.
///
/// One field carries the whole state machine. `Some` means "already warned about
/// this episode of low disk"; clearing it on recovery is what re-arms the edge.
/// A separate `armed` boolean would say the same thing twice and let the two
/// disagree.
struct LowDiskTrigger {
    last_nudge_ms: Option<u64>,
}

impl LowDiskTrigger {
    /// Whether this reading is a fresh crossing worth notifying about.
    ///
    /// Re-arming lives here because it depends only on the reading. Marking the
    /// notification as delivered does not, and is deliberately left to
    /// [`Self::mark_fired`]: the delivery gate downstream can still suppress the
    /// notification (notifications off), and marking a suppressed notification
    /// as fired would leave the trigger claiming it had warned about an episode
    /// the reader never saw. Switching notifications back on while the disk was
    /// still full would then stay silent until the disk recovered and filled
    /// again.
    ///
    /// This is also why the tick calls it unconditionally rather than skipping
    /// the whole block when notifications are off: the re-arm above is part of
    /// reading the disk, not part of delivering a notification, and a recovery
    /// that happened while the preference was off still has to count.
    fn should_nudge(&mut self, free_gb: u64, threshold_gb: u32) -> bool {
        let rearm_at = u64::from(threshold_gb).saturating_add(REARM_HYSTERESIS_GB);
        if free_gb >= rearm_at {
            self.last_nudge_ms = None;
            return false;
        }
        is_low(free_gb, threshold_gb) && self.last_nudge_ms.is_none()
    }

    /// Record that the reader has now been warned about this episode.
    ///
    /// `now_ms` is stored but never compared against: a generic cooldown window
    /// is not the gate here, because re-arming is what governs repeats, and it
    /// is a much longer-lived condition than any cooldown would be. The
    /// timestamp exists so the persisted row can seed a fresh launch.
    fn mark_fired(&mut self, now_ms: u64) {
        self.last_nudge_ms = Some(now_ms);
    }
}

/// Deliver the low-disk notification, subject to the reader's preferences.
/// Reports whether the episode was consumed: either shown now, or deliberately
/// dropped in a way that must not be retried.
///
/// The trigger has already decided this is a genuine crossing; what's left is
/// the same delivery gate every other notification uses, which is why the whole
/// decision lives in [`crate::notifications`] rather than here. A `false` means
/// the edge is still pending and the next poll — a minute later, on a disk just
/// as full — should try again.
fn nudge_low_disk(app: &AppHandle, free_gb: u64, threshold_gb: u32) -> bool {
    crate::notifications::note_disk_space_low(app, free_gb, threshold_gb)
}

/// Free space on the startup volume, in bytes.
///
/// Uses `volumeAvailableCapacityForImportantUsage`, which is what Finder's "N GB
/// available" reports: it counts purgeable space (caches, local Time Machine
/// snapshots) that macOS will reclaim under pressure. `statfs` would report a
/// smaller, more literal number — technically true and consistently alarming,
/// since a Mac routinely holds tens of gigabytes of purgeable data. The point is
/// to match what the reader sees everywhere else.
#[cfg(target_os = "macos")]
fn free_bytes() -> Option<u64> {
    use objc2_foundation::{
        NSArray, NSNumber, NSString, NSURL, NSURLVolumeAvailableCapacityForImportantUsageKey,
    };

    let url = NSURL::fileURLWithPath(&NSString::from_str("/"));
    let key = unsafe { NSURLVolumeAvailableCapacityForImportantUsageKey };
    let values = url
        .resourceValuesForKeys_error(&NSArray::from_slice(&[key]))
        .ok()?;
    let capacity = values.objectForKey(key)?;
    let bytes = capacity.downcast_ref::<NSNumber>()?.longLongValue();
    u64::try_from(bytes).ok()
}

#[cfg(not(target_os = "macos"))]
fn free_bytes() -> Option<u64> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trigger() -> LowDiskTrigger {
        LowDiskTrigger {
            last_nudge_ms: None,
        }
    }

    #[test]
    fn title_follows_the_display_preference() {
        assert_eq!(
            title_for(DiskSpaceDisplay::Always, 400, 50),
            Some("400 GB".to_string())
        );
        assert_eq!(title_for(DiskSpaceDisplay::WhenLow, 400, 50), None);
        assert_eq!(
            title_for(DiskSpaceDisplay::WhenLow, 12, 50),
            Some("12 GB".to_string())
        );
        assert_eq!(title_for(DiskSpaceDisplay::Never, 12, 50), None);
    }

    #[test]
    fn the_threshold_itself_is_not_low() {
        assert!(!is_low(50, 50));
        assert!(is_low(49, 50));
    }

    /// One poll of the real loop: decide, then mark only if delivery succeeded.
    /// `delivered` stands in for the gate inside `nudge_low_disk`.
    fn poll(trigger: &mut LowDiskTrigger, free_gb: u64, now_ms: u64, delivered: bool) -> bool {
        let wants = trigger.should_nudge(free_gb, 50);
        if wants && delivered {
            trigger.mark_fired(now_ms);
        }
        wants && delivered
    }

    #[test]
    fn nudges_once_on_the_way_down_and_stays_quiet() {
        let mut trigger = trigger();
        assert!(!poll(&mut trigger, 80, 1_000, true));
        assert!(poll(&mut trigger, 48, 2_000, true));
        // Still low on every subsequent poll — but not news any more.
        assert!(!poll(&mut trigger, 47, 3_000, true));
        assert!(!poll(&mut trigger, 20, 4_000, true));
    }

    #[test]
    fn recovering_past_the_hysteresis_re_arms_the_nudge() {
        let mut trigger = trigger();
        assert!(poll(&mut trigger, 48, 1_000, true));
        // Back over the line, but inside the hysteresis band: not a real
        // recovery, so crossing down again must not re-notify.
        assert!(!poll(&mut trigger, 52, 2_000, true));
        assert!(!poll(&mut trigger, 48, 3_000, true));
        // A genuine cleanup clears the band and re-arms.
        assert!(!poll(&mut trigger, 55, 4_000, true));
        assert!(poll(&mut trigger, 48, 5_000, true));
    }

    #[test]
    fn a_preference_blocked_nudge_retries_when_delivery_returns() {
        let mut trigger = trigger();
        // Notifications disabled in antiburn prevents the delivery decision
        // from running. The crossing is real, but the episode remains pending
        // so it can retry.
        assert!(!poll(&mut trigger, 48, 1_000, false));
        assert!(!poll(&mut trigger, 47, 2_000, false));
        // Switched back on, disk still full: this is the first warning the
        // reader actually gets, and it has to arrive without waiting for a
        // recovery.
        assert!(poll(&mut trigger, 46, 3_000, true));
        assert!(!poll(&mut trigger, 45, 4_000, true));
    }

    /// A recovery that happens while notifications are off still counts, which
    /// is why the tick runs `should_nudge` every pass rather than skipping the
    /// block when the preference is off.
    #[test]
    fn a_recovery_while_notifications_are_off_still_re_arms() {
        let mut trigger = trigger();
        assert!(poll(&mut trigger, 48, 1_000, true));
        assert!(!poll(&mut trigger, 55, 2_000, false));
        assert!(poll(&mut trigger, 48, 3_000, true));
    }

    #[test]
    fn a_seeded_trigger_does_not_re_nudge_on_relaunch() {
        // What `spawn_disk_monitor` builds when the stored stamp says we
        // already warned about this episode.
        let mut trigger = LowDiskTrigger {
            last_nudge_ms: Some(1_000),
        };
        assert!(!trigger.should_nudge(20, 50));
    }

    #[test]
    fn free_space_is_reported_in_truncated_binary_gb() {
        assert_eq!(format_free(50 * BYTES_PER_GB / BYTES_PER_GB), "50 GB");
        // Truncating integer division: 49.9 GB reads as 49, never 50.
        assert_eq!((50 * BYTES_PER_GB - 1) / BYTES_PER_GB, 49);
    }

    /// The stamp is only ever a seed, so the key it is stored under is part of
    /// the store contract rather than an implementation detail.
    #[test]
    fn the_persisted_stamp_is_an_internal_row_not_a_preference() {
        assert!(LAST_FIRED_KEY.starts_with("internal:"));
    }

    #[test]
    fn the_poll_stays_out_of_the_way_of_launch() {
        // Shorter than the update scheduler's, which makes a network call.
        assert!(STARTUP_DELAY < crate::updates::STARTUP_DELAY);
        assert_eq!(DISK_POLL, Duration::from_secs(60));
    }
}
