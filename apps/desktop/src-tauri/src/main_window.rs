//! Shell policy for the ordinary main window.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use antiburn_main_window::Placement;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, WebviewWindow};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons};

use crate::dto::{BurnCheckSamplePayload, BurnCheckSampleSurface, OpenBurnCheckSampleOutcome};
use crate::remediation::BurnCheckSampleSession;
use crate::store::{SessionKey, Store};
use crate::window_lifecycle::{self, ManagedWindowReadiness};
use crate::window_readiness::{
    HealthAckAction, RetainedOpenAction, TerminalRetry, WindowReadiness, renderer_generation_script,
};

pub use antiburn_main_window::LABEL;

/// Event carrying whether the retained renderer can present work.
pub const VISIBILITY_CHANGED_EVENT: &str = "main:visibility-changed";

/// Event carrying the latest session requested for the main window.
pub const SESSION_TARGET_EVENT: &str = "main:session-target";

/// Event carrying the latest requested main-window section.
pub const SECTION_TARGET_EVENT: &str = "main:section-target";

const SAMPLE_HANDLE_TTL: Duration = Duration::from_secs(10 * 60);
const SAMPLE_HANDLE_LIMIT: usize = 100;
const MISSING_SAMPLE_TITLE: &str = "Untitled session";

/// A destination in the retained main window.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum MainWindowSection {
    Activity,
    BurnChecks,
}

/// Revisioned section request shared by event and cold-renderer paths.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SectionTargetRequest {
    revision: u64,
    section: MainWindowSection,
}
/// Event asking the retained renderer to report its committed health.
pub const HEALTH_CHECK_EVENT: &str = "main:health-check";

/// This timeout remains provisional until hidden-open measurements calibrate it.
pub const HEALTH_ACK_TIMEOUT: Duration = Duration::from_millis(300);

/// A replacement must report readiness within this provisional bound.
pub const RECOVERY_LOAD_TIMEOUT: Duration = Duration::from_secs(10);

const MAX_CONSECUTIVE_AUTOMATIC_RECOVERIES: u32 = 2;
const MAX_FAILURE_REPORTS_PER_GENERATION: u32 = 5;
const PLACEMENT_KEY: &str = "internal:mainWindowPlacementV1";
const PLACEMENT_WRITE_DELAY: Duration = Duration::from_millis(350);

/// Identity-only target for one local session.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionTarget {
    agent: String,
    session_id: String,
    wsl_distro: Option<String>,
}

/// Revisioned request shared by the event and renderer peek paths.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionTargetRequest {
    revision: u64,
    target: SessionTarget,
}

#[derive(Clone, Debug)]
struct SampleTarget {
    handle: String,
    target: SessionTarget,
    created_at: Instant,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SampleTargetError {
    Expired,
    Unavailable,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RendererStatus {
    Healthy,
    Fallback,
    Degraded,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RenderFailureKind {
    RenderFallback,
    WindowError,
    UnhandledRejection,
    ResponderInstallFailed,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RenderFailureCategory {
    TypeError,
    ReferenceError,
    RangeError,
    SyntaxError,
    DomException,
    InvokeError,
    NonErrorValue,
    Unknown,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
pub enum RenderErrorName {
    Error,
    TypeError,
    ReferenceError,
    RangeError,
    SyntaxError,
    EvalError,
    #[serde(rename = "URIError")]
    UriError,
    AggregateError,
    #[serde(rename = "DOMException")]
    DomException,
    Other,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RenderFailureReport {
    kind: RenderFailureKind,
    category: RenderFailureCategory,
    error_name: RenderErrorName,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthCheckRequest {
    request_id: u64,
    generation: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LoadFailureCause {
    DestroyFailed,
    BuildFailed,
    ReplacementHung,
    VerificationTimeout,
    FallbackAck,
}

impl LoadFailureCause {
    const fn as_str(self) -> &'static str {
        match self {
            Self::DestroyFailed => "destroy_failed",
            Self::BuildFailed => "build_failed",
            Self::ReplacementHung => "replacement_hung",
            Self::VerificationTimeout => "verification_timeout",
            Self::FallbackAck => "fallback_ack",
        }
    }

    const fn verifies_ready(self) -> bool {
        matches!(self, Self::VerificationTimeout | Self::FallbackAck)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum DialogState {
    #[default]
    None,
    Pending,
    Open {
        token: u64,
    },
}

#[derive(Debug, Default)]
struct RecoveryLedger {
    consecutive_failures: u32,
    dialog: DialogState,
    next_dialog_token: u64,
}

#[derive(Debug, Default)]
struct SessionTargetState {
    pending: Option<SessionTargetRequest>,
    applied: Option<(u64, u64)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TargetAck {
    RetiredNow,
    RecordedRetained,
    StaleRevision,
    StaleGeneration,
    AlreadyRetired,
}

impl TargetAck {
    const fn as_str(self) -> &'static str {
        match self {
            Self::RetiredNow => "retired_now",
            Self::RecordedRetained => "recorded_retained",
            Self::StaleRevision => "stale_revision",
            Self::StaleGeneration => "stale_generation",
            Self::AlreadyRetired => "already_retired",
        }
    }
}

#[cfg(target_os = "macos")]
static APPLICATION_HIDDEN: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Observe visibility changes that do not produce Tauri focus events.
#[cfg(target_os = "macos")]
pub fn install_visibility_observers(app: &AppHandle) {
    use block2::RcBlock;
    use objc2::MainThreadMarker;
    use objc2_app_kit::{
        NSApplication, NSApplicationDidHideNotification, NSApplicationDidUnhideNotification,
        NSWindowDidDeminiaturizeNotification, NSWindowDidMiniaturizeNotification,
    };
    use objc2_foundation::{NSNotification, NSNotificationCenter};

    if let Some(main_thread) = MainThreadMarker::new() {
        APPLICATION_HIDDEN.store(
            NSApplication::sharedApplication(main_thread).isHidden(),
            Ordering::Release,
        );
    }
    let center = NSNotificationCenter::defaultCenter();
    // SAFETY: AppKit provides these immutable names on all supported macOS versions.
    let notifications = unsafe {
        [
            (NSApplicationDidHideNotification, Some(true)),
            (NSApplicationDidUnhideNotification, Some(false)),
            (NSWindowDidMiniaturizeNotification, None),
            (NSWindowDidDeminiaturizeNotification, None),
        ]
    };
    for (name, hidden) in notifications {
        let app = app.clone();
        let handler = RcBlock::new(move |_notification: core::ptr::NonNull<NSNotification>| {
            if let Some(hidden) = hidden {
                APPLICATION_HIDDEN.store(hidden, Ordering::Release);
            }
            if let Some(window) = app.get_webview_window(LABEL) {
                emit_visibility_changed(&window);
            }
        });
        // SAFETY: AppKit posts these notifications on the main thread.
        let observer = unsafe {
            center.addObserverForName_object_queue_usingBlock(Some(name), None, None, &handler)
        };
        // The observers remain registered for the app lifetime.
        std::mem::forget(observer);
    }
}

#[cfg(not(target_os = "macos"))]
pub fn install_visibility_observers(_app: &AppHandle) {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpenTrigger {
    ColdLaunch,
    Interaction,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OpenKind {
    ColdLaunch,
    FirstOpen,
    WarmReopen,
}

impl OpenKind {
    const fn as_str(self) -> &'static str {
        match self {
            Self::ColdLaunch => "cold_launch",
            Self::FirstOpen => "first_open",
            Self::WarmReopen => "warm_reopen",
        }
    }
}

#[derive(Debug, Default)]
struct Presentation {
    has_revealed: bool,
    restore_after_activation: bool,
    pending: Option<(OpenKind, Instant)>,
}

/// Renderer, presentation, recovery, target, and placement state.
pub struct MainWindowState {
    readiness: Mutex<WindowReadiness>,
    presentation: Mutex<Presentation>,
    placement: Mutex<Option<Placement>>,
    placement_generation: AtomicU64,
    recovery: Mutex<RecoveryLedger>,
    renderer_status: Mutex<Option<(u64, RendererStatus)>>,
    failure_reports: Mutex<(u64, u32)>,
    session_target: Mutex<SessionTargetState>,
    session_target_revision: AtomicU64,
    section_target: Mutex<Option<SectionTargetRequest>>,
    section_target_revision: AtomicU64,
    sample_targets: Mutex<VecDeque<SampleTarget>>,
}

impl MainWindowState {
    pub fn load(store: &Store) -> Self {
        Self {
            readiness: Mutex::new(WindowReadiness::default()),
            presentation: Mutex::new(Presentation::default()),
            placement: Mutex::new(
                store
                    .internal_value(PLACEMENT_KEY)
                    .and_then(|raw| serde_json::from_str(&raw).ok()),
            ),
            placement_generation: AtomicU64::new(0),
            recovery: Mutex::new(RecoveryLedger::default()),
            renderer_status: Mutex::new(None),
            failure_reports: Mutex::new((0, 0)),
            session_target: Mutex::new(SessionTargetState::default()),
            session_target_revision: AtomicU64::new(0),
            section_target: Mutex::new(None),
            section_target_revision: AtomicU64::new(0),
            sample_targets: Mutex::new(VecDeque::new()),
        }
    }

    pub fn issue_sample_handle(
        &self,
        agent: String,
        session_id: String,
        wsl_distro: Option<String>,
        now: Instant,
    ) -> Result<String, String> {
        let mut bytes = [0_u8; 16];
        getrandom::fill(&mut bytes).map_err(|_| "unable to create sample handle".to_owned())?;
        let handle: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
        let mut targets = lock(&self.sample_targets);
        targets
            .retain(|entry| now.saturating_duration_since(entry.created_at) <= SAMPLE_HANDLE_TTL);
        while targets.len() >= SAMPLE_HANDLE_LIMIT {
            targets.pop_front();
        }
        targets.push_back(SampleTarget {
            handle: handle.clone(),
            target: SessionTarget {
                agent,
                session_id,
                wsl_distro,
            },
            created_at: now,
        });
        Ok(handle)
    }

    pub fn resolve_sample_handle(
        &self,
        handle: &str,
        now: Instant,
    ) -> Result<SessionTarget, SampleTargetError> {
        let targets = lock(&self.sample_targets);
        let entry = targets
            .iter()
            .find(|entry| entry.handle == handle)
            .ok_or(SampleTargetError::Unavailable)?;
        if now.saturating_duration_since(entry.created_at) > SAMPLE_HANDLE_TTL {
            return Err(SampleTargetError::Expired);
        }
        Ok(entry.target.clone())
    }

    fn request_session_target(&self, target: SessionTarget) -> SessionTargetRequest {
        let request = SessionTargetRequest {
            revision: next_atomic_nonzero(&self.session_target_revision),
            target,
        };
        let mut state = lock(&self.session_target);
        state.pending = Some(request.clone());
        state.applied = None;
        request
    }

    fn clear_session_target(&self, revision: u64) {
        let mut state = lock(&self.session_target);
        if state
            .pending
            .as_ref()
            .is_some_and(|request| request.revision == revision)
        {
            state.pending = None;
            state.applied = None;
        }
    }

    fn peek_session_target(
        &self,
        caller_generation: u64,
        active_generation: Option<u64>,
    ) -> Option<SessionTargetRequest> {
        if active_generation != Some(caller_generation) {
            return None;
        }
        lock(&self.session_target).pending.clone()
    }

    fn record_target_applied(
        &self,
        generation: u64,
        revision: u64,
        active_generation: Option<u64>,
        ready_generation: Option<u64>,
        presented: bool,
    ) -> TargetAck {
        if active_generation != Some(generation) {
            return TargetAck::StaleGeneration;
        }
        let mut state = lock(&self.session_target);
        let Some(pending) = state.pending.as_ref() else {
            return TargetAck::AlreadyRetired;
        };
        if pending.revision != revision {
            return TargetAck::StaleRevision;
        }
        state.applied = Some((generation, revision));
        if ready_generation == Some(generation) && presented {
            state.pending = None;
            state.applied = None;
            TargetAck::RetiredNow
        } else {
            TargetAck::RecordedRetained
        }
    }

    fn retire_target_on_reveal(&self, revealed_generation: u64) -> bool {
        let mut state = lock(&self.session_target);
        let Some(revision) = state.pending.as_ref().map(|request| request.revision) else {
            return false;
        };
        if state.applied != Some((revealed_generation, revision)) {
            return false;
        }
        state.pending = None;
        state.applied = None;
        true
    }

    fn invalidate_target_application(&self, doomed_generation: u64) {
        let mut state = lock(&self.session_target);
        if state
            .applied
            .is_some_and(|(generation, _)| generation == doomed_generation)
        {
            state.applied = None;
        }
    }

    fn request_section_target(&self, section: MainWindowSection) -> SectionTargetRequest {
        let request = SectionTargetRequest {
            revision: self
                .section_target_revision
                .fetch_add(1, Ordering::AcqRel)
                .wrapping_add(1),
            section,
        };
        *lock(&self.section_target) = Some(request.clone());
        request
    }

    fn take_section_target(&self) -> Option<SectionTargetRequest> {
        lock(&self.section_target).take()
    }

    fn clear_section_target(&self, revision: u64) {
        let mut target = lock(&self.section_target);
        if target
            .as_ref()
            .is_some_and(|request| request.revision == revision)
        {
            *target = None;
        }
    }

    fn note_open_request(&self, trigger: OpenTrigger, now: Instant) -> OpenKind {
        let mut presentation = lock(&self.presentation);
        let kind = if presentation.has_revealed {
            OpenKind::WarmReopen
        } else if trigger == OpenTrigger::ColdLaunch {
            OpenKind::ColdLaunch
        } else {
            OpenKind::FirstOpen
        };
        presentation.pending.get_or_insert((kind, now)).0
    }

    fn cancel_open_request(&self) {
        lock(&self.presentation).pending = None;
    }

    fn note_closed(&self) {
        let mut presentation = lock(&self.presentation);
        presentation.restore_after_activation = presentation.has_revealed;
    }

    #[cfg(any(target_os = "macos", test))]
    fn should_restore_after_activation(
        &self,
        main_visible: bool,
        main_minimized: bool,
        another_window_owns_activation: bool,
    ) -> bool {
        let presentation = lock(&self.presentation);
        presentation.has_revealed
            && (main_minimized || (presentation.restore_after_activation && !main_visible))
            && !another_window_owns_activation
    }

    fn finish_reveal(&self, now: Instant) -> (OpenKind, Duration) {
        let mut presentation = lock(&self.presentation);
        presentation.has_revealed = true;
        presentation.restore_after_activation = false;
        let (kind, requested_at) = presentation
            .pending
            .take()
            .unwrap_or((OpenKind::WarmReopen, now));
        (kind, now.saturating_duration_since(requested_at))
    }

    fn request_details(&self, now: Instant) -> (OpenKind, Duration) {
        let presentation = lock(&self.presentation);
        let (kind, requested_at) = presentation.pending.unwrap_or((OpenKind::WarmReopen, now));
        (kind, now.saturating_duration_since(requested_at))
    }

    fn placement(&self) -> Option<Placement> {
        lock(&self.placement).clone()
    }

    fn remember_placement(&self, applied: Option<Placement>) {
        let mut placement = lock(&self.placement);
        if placement.is_none() {
            *placement = applied;
        }
    }

    fn accept_renderer_status(&self, generation: u64, status: RendererStatus) -> bool {
        if self.readiness().active_generation() != Some(generation) {
            return false;
        }
        *lock(&self.renderer_status) = Some((generation, status));
        if status == RendererStatus::Healthy {
            lock(&self.recovery).consecutive_failures = 0;
        }
        true
    }

    fn accept_failure_report(&self, generation: u64) -> bool {
        if self.readiness().active_generation() != Some(generation) {
            return false;
        }
        let mut reports = lock(&self.failure_reports);
        if reports.0 != generation {
            *reports = (generation, 0);
        }
        if reports.1 >= MAX_FAILURE_REPORTS_PER_GENERATION {
            return false;
        }
        reports.1 += 1;
        true
    }

    fn increment_failures(&self) -> u32 {
        let mut recovery = lock(&self.recovery);
        recovery.consecutive_failures = recovery.consecutive_failures.saturating_add(1);
        recovery.consecutive_failures
    }

    fn defer_or_open_dialog(&self, reveal_was_pending: bool) -> bool {
        if reveal_was_pending {
            true
        } else {
            lock(&self.recovery).dialog = DialogState::Pending;
            false
        }
    }

    fn begin_dialog(&self) -> Option<u64> {
        let mut recovery = lock(&self.recovery);
        if matches!(recovery.dialog, DialogState::Open { .. }) {
            return None;
        }
        let token = next_nonzero(&mut recovery.next_dialog_token);
        recovery.dialog = DialogState::Open { token };
        Some(token)
    }

    fn consume_dialog(&self, token: u64) -> bool {
        let mut recovery = lock(&self.recovery);
        if recovery.dialog != (DialogState::Open { token }) {
            return false;
        }
        recovery.dialog = DialogState::None;
        true
    }

    fn reset_after_informed_retry(&self) {
        lock(&self.recovery).consecutive_failures = 0;
        *lock(&self.renderer_status) = None;
        *lock(&self.failure_reports) = (0, 0);
    }
}

impl ManagedWindowReadiness for MainWindowState {
    fn readiness(&self) -> MutexGuard<'_, WindowReadiness> {
        lock(&self.readiness)
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn next_nonzero(value: &mut u64) -> u64 {
    *value = value.wrapping_add(1);
    if *value == 0 {
        *value = 1;
    }
    *value
}

fn next_atomic_nonzero(value: &AtomicU64) -> u64 {
    loop {
        let current = value.load(Ordering::Acquire);
        let next = if current == u64::MAX { 1 } else { current + 1 };
        if value
            .compare_exchange(current, next, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            return next;
        }
    }
}

#[cfg(target_os = "macos")]
fn debug_assert_main_thread() {
    debug_assert!(objc2::MainThreadMarker::new().is_some());
}

#[cfg(not(target_os = "macos"))]
fn debug_assert_main_thread() {}

/// Run one lifecycle decision and its native effect on the main thread.
pub(crate) fn on_main(app: &AppHandle, task: impl FnOnce(&AppHandle) + Send + 'static) {
    let handle = app.clone();
    if let Err(error) = app.run_on_main_thread(move || {
        debug_assert_main_thread();
        task(&handle);
    }) {
        ::tracing::debug!(event = "main_window_dispatch_failed", error = %error);
    }
}

/// Compute one command value on the main thread without blocking it.
async fn on_main_value<T: Send + 'static>(
    app: &AppHandle,
    task: impl FnOnce(&AppHandle) -> T + Send + 'static,
) -> Result<T, String> {
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let handle = app.clone();
    app.run_on_main_thread(move || {
        debug_assert_main_thread();
        let _ = sender.send(task(&handle));
    })
    .map_err(|error| error.to_string())?;
    receiver
        .await
        .map_err(|_| "the application stopped before the main-window command ran".to_owned())
}

/// Open the main window or coalesce with its active renderer load.
pub fn open(app: &AppHandle, trigger: OpenTrigger) -> tauri::Result<()> {
    debug_assert_main_thread();
    let now = Instant::now();
    let state = app.state::<MainWindowState>();
    let open_kind = state.note_open_request(trigger, now);
    ::tracing::info!(
        event = "main_window_open_requested",
        window = LABEL,
        open_kind = open_kind.as_str()
    );

    let presented = app
        .get_webview_window(LABEL)
        .as_ref()
        .is_some_and(is_visible);
    let action = state.readiness().request_open_retained(now, presented);
    match action {
        RetainedOpenAction::Reveal => reveal(app.get_webview_window(LABEL).as_ref()),
        RetainedOpenAction::AwaitReady | RetainedOpenAction::AwaitVerification => Ok(()),
        RetainedOpenAction::StartLoading { generation } => {
            if build(app, generation).is_err() {
                resolve_load_failure(app, generation, LoadFailureCause::BuildFailed);
            }
            Ok(())
        }
        RetainedOpenAction::Rebuild { generation } => {
            arm_recovery_watchdog(app, generation);
            let Some(window) = app.get_webview_window(LABEL) else {
                if build(app, generation).is_err() {
                    resolve_load_failure(app, generation, LoadFailureCause::BuildFailed);
                }
                return Ok(());
            };
            if !state.readiness().defer_build_until_destroyed(generation) {
                return Ok(());
            }
            if window.destroy().is_err() {
                resolve_load_failure(app, generation, LoadFailureCause::DestroyFailed);
            }
            Ok(())
        }
        RetainedOpenAction::Verify {
            generation,
            request_id,
        } => {
            let (open_kind, elapsed) = state.request_details(now);
            ::tracing::info!(
                event = "main_window_health_check_started",
                window = LABEL,
                generation,
                request_id,
                open_kind = open_kind.as_str(),
                elapsed_ms = elapsed.as_millis() as u64
            );
            let request = HealthCheckRequest {
                request_id,
                generation,
            };
            if let Err(error) = app.emit_to(LABEL, HEALTH_CHECK_EVENT, request) {
                ::tracing::debug!(event = "main_window_health_check_emit_failed", error = %error);
            }
            arm_health_timeout(app, generation, request_id);
            Ok(())
        }
        RetainedOpenAction::AttendTerminal => {
            show_terminal_dialog(app);
            Ok(())
        }
    }
}

/// Open the main window and route its renderer to one exact session.
#[tauri::command]
pub fn open_main_window_session(app: AppHandle, target: SessionTarget) -> Result<(), String> {
    route_session_target(&app, target)
}

fn route_session_target(app: &AppHandle, target: SessionTarget) -> Result<(), String> {
    let state = app.state::<MainWindowState>();
    let request = state.request_session_target(target);
    let section_request = state.request_section_target(MainWindowSection::Activity);
    if let Err(error) = open(app, OpenTrigger::Interaction) {
        state.clear_session_target(request.revision);
        state.clear_section_target(section_request.revision);
        return Err(error.to_string());
    }
    app.emit_to(LABEL, SECTION_TARGET_EVENT, section_request)
        .and_then(|()| app.emit_to(LABEL, SESSION_TARGET_EVENT, request))
        .map_err(|error| error.to_string())
}

/// Mint bounded renderer samples while retaining exact identities in Rust.
pub fn sample_payloads(
    app: &AppHandle,
    samples: &[BurnCheckSampleSession],
) -> Result<Vec<BurnCheckSamplePayload>, String> {
    let state = app.state::<MainWindowState>();
    let store = app.state::<Store>();
    let now = Instant::now();
    samples
        .iter()
        .filter_map(|sample| {
            let key = SessionKey::new(
                sample.environment_key.clone(),
                sample.agent.clone(),
                sample.session_id.clone(),
            );
            let record = match store.session(&key) {
                Ok(Some(record)) => record,
                Ok(None) => return None,
                Err(error) => return Some(Err(error.to_string())),
            };
            Some(
                state
                    .issue_sample_handle(
                        sample.agent.clone(),
                        sample.session_id.clone(),
                        record.wsl_distro,
                        now,
                    )
                    .map(|navigation_handle| BurnCheckSamplePayload {
                        navigation_handle,
                        title: sample_title(record.title.as_deref()),
                        agent: sample.agent.clone(),
                        surface: sample_surface(&record.surface),
                        observed_at_ms: sample.observed_at_ms,
                    }),
            )
        })
        .collect()
}

fn sample_title(title: Option<&str>) -> String {
    title
        .filter(|title| !title.trim().is_empty())
        .unwrap_or(MISSING_SAMPLE_TITLE)
        .to_owned()
}

fn sample_surface(surface: &str) -> BurnCheckSampleSurface {
    match surface {
        "cli" => BurnCheckSampleSurface::Cli,
        "ide_desktop" => BurnCheckSampleSurface::IdeDesktop,
        _ => BurnCheckSampleSurface::Unknown,
    }
}

/// Resolve one opaque sample route and use the standard revisioned session controller.
#[tauri::command]
pub fn open_burn_check_sample(
    window: WebviewWindow,
    app: AppHandle,
    navigation_handle: String,
) -> Result<OpenBurnCheckSampleOutcome, String> {
    if window.label() != LABEL {
        return Err("burn check samples are unavailable to this window".to_owned());
    }
    let target = match resolve_sample_for_open(
        &app.state::<MainWindowState>(),
        &app.state::<Store>(),
        &navigation_handle,
        Instant::now(),
    )? {
        Ok(target) => target,
        Err(outcome) => return Ok(outcome),
    };
    route_session_target(&app, target)?;
    Ok(OpenBurnCheckSampleOutcome::Opened)
}

fn resolve_sample_for_open(
    state: &MainWindowState,
    store: &Store,
    handle: &str,
    now: Instant,
) -> Result<Result<SessionTarget, OpenBurnCheckSampleOutcome>, String> {
    let target = match state.resolve_sample_handle(handle, now) {
        Ok(target) => target,
        Err(SampleTargetError::Expired) => {
            return Ok(Err(OpenBurnCheckSampleOutcome::Expired));
        }
        Err(SampleTargetError::Unavailable) => {
            return Ok(Err(OpenBurnCheckSampleOutcome::Unavailable));
        }
    };
    let exists = store
        .session(&SessionKey::for_session(
            &target.agent,
            &target.session_id,
            target.wsl_distro.as_deref(),
        ))
        .map_err(|error| error.to_string())?
        .is_some();
    if exists {
        Ok(Ok(target))
    } else {
        Ok(Err(OpenBurnCheckSampleOutcome::Deleted))
    }
}

/// Open the main window and route its renderer to a top-level section.
#[tauri::command]
pub fn open_main_window_section(
    window: WebviewWindow,
    app: AppHandle,
    section: MainWindowSection,
) -> Result<(), String> {
    if window.label() != crate::popover::LABEL {
        return Err("main-window sections are unavailable to this window".to_owned());
    }
    let state = app.state::<MainWindowState>();
    let request = state.request_section_target(section);
    if let Err(error) = open(&app, OpenTrigger::Interaction) {
        state.clear_section_target(request.revision);
        return Err(error.to_string());
    }
    app.emit_to(LABEL, SECTION_TARGET_EVENT, request)
        .map_err(|error| error.to_string())
}

/// Read the latest target without retiring the upstream acknowledgment lifecycle.
#[tauri::command]
pub fn take_main_window_session_target(
    window: WebviewWindow,
) -> Result<Option<SessionTargetRequest>, String> {
    if window.label() != LABEL {
        return Err("main-window session targets are unavailable to this window".to_owned());
    }
    Ok(lock(
        &window
            .app_handle()
            .state::<MainWindowState>()
            .session_target,
    )
    .pending
    .clone())
}

/// Peek at the latest target after the main renderer installs its listener.
#[tauri::command]
pub async fn peek_main_window_session_target(
    window: WebviewWindow,
    generation: u64,
) -> Result<Option<SessionTargetRequest>, String> {
    if window.label() != LABEL {
        return Err("main-window session targets are unavailable to this window".to_owned());
    }
    let app = window.app_handle().clone();
    on_main_value(&app, move |app| {
        let state = app.try_state::<MainWindowState>()?;
        let active = state.readiness().active_generation();
        state.peek_session_target(generation, active)
    })
    .await
}

#[tauri::command]
pub fn acknowledge_main_window_session_target(
    window: WebviewWindow,
    generation: u64,
    revision: u64,
) -> Result<(), String> {
    if window.label() != LABEL {
        return Err("main-window session targets are unavailable to this window".to_owned());
    }
    let app = window.app_handle().clone();
    on_main(&app, move |app| {
        let Some(state) = app.try_state::<MainWindowState>() else {
            return;
        };
        let (active, ready) = {
            let readiness = state.readiness();
            (readiness.active_generation(), readiness.ready_generation())
        };
        let presented = app
            .get_webview_window(LABEL)
            .as_ref()
            .is_some_and(is_visible);
        let outcome = state.record_target_applied(generation, revision, active, ready, presented);
        ::tracing::info!(
            event = "main_window_target_ack",
            window = LABEL,
            generation,
            revision,
            outcome = outcome.as_str()
        );
    });
    Ok(())
}

#[tauri::command]
pub fn main_window_health_ack(
    window: WebviewWindow,
    request_id: u64,
    generation: u64,
    healthy: bool,
) -> Result<(), String> {
    if window.label() != LABEL {
        return Err("main-window health is unavailable to this window".to_owned());
    }
    let app = window.app_handle().clone();
    on_main(&app, move |app| {
        let Some(state) = app.try_state::<MainWindowState>() else {
            return;
        };
        let action = state
            .readiness()
            .health_ack(request_id, generation, healthy);
        match action {
            HealthAckAction::ArmReveal => {
                ::tracing::info!(
                    event = "main_window_health_ack",
                    window = LABEL,
                    generation,
                    request_id,
                    healthy = true
                );
                if state.readiness().take_armed_reveal(generation)
                    && let Err(error) = reveal(app.get_webview_window(LABEL).as_ref())
                {
                    ::tracing::error!(event = "window_reveal_failed", window = LABEL, error = %error);
                }
            }
            HealthAckAction::RecoverEligible => {
                ::tracing::info!(
                    event = "main_window_health_ack",
                    window = LABEL,
                    generation,
                    request_id,
                    healthy = false
                );
                resolve_load_failure(app, generation, LoadFailureCause::FallbackAck);
            }
            HealthAckAction::None => {
                ::tracing::debug!(
                    event = "main_window_health_ack_ignored",
                    window = LABEL,
                    generation,
                    request_id
                );
            }
        }
    });
    Ok(())
}

#[tauri::command]
pub async fn main_window_pending_health_check(
    window: WebviewWindow,
    generation: u64,
) -> Result<Option<HealthCheckRequest>, String> {
    if window.label() != LABEL {
        return Err("main-window health is unavailable to this window".to_owned());
    }
    let app = window.app_handle().clone();
    on_main_value(&app, move |app| {
        let state = app.try_state::<MainWindowState>()?;
        let (request_id, active_generation) = state.readiness().pending_health_check()?;
        (active_generation == generation).then_some(HealthCheckRequest {
            request_id,
            generation: active_generation,
        })
    })
    .await
}

#[tauri::command]
pub fn report_main_window_render_status(
    window: WebviewWindow,
    generation: u64,
    status: RendererStatus,
) -> Result<(), String> {
    if window.label() != LABEL {
        return Err("main-window status is unavailable to this window".to_owned());
    }
    let app = window.app_handle().clone();
    on_main(&app, move |app| {
        let Some(state) = app.try_state::<MainWindowState>() else {
            return;
        };
        if !state.accept_renderer_status(generation, status) {
            ::tracing::debug!(
                event = "main_window_render_status_ignored",
                window = LABEL,
                generation
            );
            return;
        }
        ::tracing::info!(
            event = "main_window_render_status",
            window = LABEL,
            generation,
            status = ?status
        );
    });
    Ok(())
}

#[tauri::command]
pub fn report_main_window_render_failure(
    window: WebviewWindow,
    generation: u64,
    report: RenderFailureReport,
) -> Result<(), String> {
    if window.label() != LABEL {
        return Err("main-window diagnostics are unavailable to this window".to_owned());
    }
    let app = window.app_handle().clone();
    on_main(&app, move |app| {
        let Some(state) = app.try_state::<MainWindowState>() else {
            return;
        };
        if !state.accept_failure_report(generation) {
            ::tracing::debug!(
                event = "main_window_render_failure_dropped",
                window = LABEL,
                generation
            );
            return;
        }
        if report.kind == RenderFailureKind::ResponderInstallFailed {
            let _ = state.accept_renderer_status(generation, RendererStatus::Degraded);
        }
        ::tracing::warn!(
            event = "main_window_render_failure",
            window = LABEL,
            generation,
            kind = ?report.kind,
            category = ?report.category,
            error_name = ?report.error_name
        );
    });
    Ok(())
}

#[tauri::command]
pub fn request_main_window_recovery(window: WebviewWindow, generation: u64) -> Result<(), String> {
    if window.label() != LABEL {
        return Err("main-window recovery is unavailable to this window".to_owned());
    }
    let app = window.app_handle().clone();
    on_main(&app, move |app| {
        let Some(state) = app.try_state::<MainWindowState>() else {
            return;
        };
        let Some(replacement) = state.readiness().begin_recovery(generation, Instant::now()) else {
            return;
        };
        start_recovery_replacement(app, generation, replacement);
    });
    Ok(())
}

/// Take the latest section target after the main renderer installs its listener.
#[tauri::command]
pub fn take_main_window_section_target(
    window: WebviewWindow,
) -> Result<Option<SectionTargetRequest>, String> {
    if window.label() != LABEL {
        return Err("main-window section targets are unavailable to this window".to_owned());
    }
    Ok(window.state::<MainWindowState>().take_section_target())
}

fn build(app: &AppHandle, generation: u64) -> tauri::Result<()> {
    let state = app.state::<MainWindowState>();
    let placement = state.placement();
    let (open_kind, elapsed) = state.request_details(Instant::now());
    ::tracing::info!(
        event = "main_window_create_started",
        window = LABEL,
        generation,
        open_kind = open_kind.as_str(),
        elapsed_ms = elapsed.as_millis() as u64
    );
    window_lifecycle::arm_stale_warning::<MainWindowState>(app, generation, LABEL);
    let built = antiburn_main_window::build(
        app,
        renderer_generation_script(generation),
        placement.as_ref(),
        |window, payload| {
            window_lifecycle::trace_page_load::<MainWindowState>(window, payload, LABEL);
        },
    )?;
    let applied = built
        .placement
        .or_else(|| antiburn_main_window::capture(&built.window, None));
    state.remember_placement(applied);
    Ok(())
}

/// Build a deferred replacement after its old label is removed.
pub fn rebuild_after_destroy(app: &AppHandle) {
    debug_assert_main_thread();
    if app.get_webview_window(LABEL).is_some() {
        ::tracing::debug!(
            event = "main_window_destroyed_stale_ignored",
            window = LABEL
        );
        return;
    }
    let generation = window_lifecycle::begin_deferred_build::<MainWindowState>(app, Instant::now());
    let Some(generation) = generation else {
        return;
    };
    if build(app, generation).is_err() {
        resolve_load_failure(app, generation, LoadFailureCause::BuildFailed);
    }
}

/// Accept readiness only from the main renderer's active generation.
pub fn renderer_ready(window: &WebviewWindow, generation: u64) {
    if window.label() != LABEL {
        return;
    }
    let app = window.app_handle().clone();
    on_main(&app, move |app| renderer_ready_on_main(app, generation));
}

fn renderer_ready_on_main(app: &AppHandle, generation: u64) {
    let Some(state) = app.try_state::<MainWindowState>() else {
        return;
    };
    if !window_lifecycle::renderer_ready::<MainWindowState>(app, LABEL, generation, Instant::now())
    {
        return;
    }
    let (open_kind, elapsed) = state.request_details(Instant::now());
    ::tracing::info!(
        event = "main_window_renderer_ready",
        window = LABEL,
        generation,
        open_kind = open_kind.as_str(),
        elapsed_ms = elapsed.as_millis() as u64
    );
    if let Err(error) = reveal(app.get_webview_window(LABEL).as_ref()) {
        ::tracing::error!(event = "window_reveal_failed", window = LABEL, error = %error);
    }
}

fn reveal(window: Option<&WebviewWindow>) -> tauri::Result<()> {
    let Some(window) = window else {
        return Ok(());
    };
    antiburn_main_window::reveal(window)?;
    let state = window.app_handle().state::<MainWindowState>();
    let (open_kind, elapsed) = state.finish_reveal(Instant::now());
    if let Some(generation) = state.readiness().ready_generation() {
        state.retire_target_on_reveal(generation);
    }
    // This marks native show and focus completion. It does not mark a painted frame.
    ::tracing::info!(
        event = "main_window_revealed",
        window = LABEL,
        open_kind = open_kind.as_str(),
        elapsed_ms = elapsed.as_millis() as u64
    );
    emit_visibility_changed(window);
    Ok(())
}

fn arm_health_timeout(app: &AppHandle, generation: u64, request_id: u64) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(HEALTH_ACK_TIMEOUT).await;
        on_main(&app, move |app| {
            let Some(state) = app.try_state::<MainWindowState>() else {
                return;
            };
            if !state.readiness().verification_timeout(request_id) {
                return;
            }
            ::tracing::warn!(
                event = "main_window_health_timeout",
                window = LABEL,
                generation,
                request_id,
                timeout_ms = HEALTH_ACK_TIMEOUT.as_millis() as u64
            );
            resolve_load_failure(app, generation, LoadFailureCause::VerificationTimeout);
        });
    });
}

fn arm_recovery_watchdog(app: &AppHandle, generation: u64) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(RECOVERY_LOAD_TIMEOUT).await;
        on_main(&app, move |app| {
            let Some(state) = app.try_state::<MainWindowState>() else {
                return;
            };
            if state.readiness().loading_generation() != Some(generation) {
                return;
            }
            ::tracing::warn!(
                event = "main_window_recovery_watchdog",
                window = LABEL,
                generation,
                timeout_ms = RECOVERY_LOAD_TIMEOUT.as_millis() as u64
            );
            resolve_load_failure(app, generation, LoadFailureCause::ReplacementHung);
        });
    });
}

fn resolve_load_failure(app: &AppHandle, generation: u64, cause: LoadFailureCause) {
    debug_assert_main_thread();
    let Some(state) = app.try_state::<MainWindowState>() else {
        return;
    };
    let current = if cause.verifies_ready() {
        state.readiness().ready_generation()
    } else {
        state.readiness().loading_generation()
    };
    if current != Some(generation) {
        ::tracing::debug!(
            event = "main_window_failure_ignored",
            window = LABEL,
            generation,
            cause = cause.as_str()
        );
        return;
    }

    let failures = state.increment_failures();
    if failures > MAX_CONSECUTIVE_AUTOMATIC_RECOVERIES {
        let reveal_was_pending = if cause.verifies_ready() {
            state.readiness().enter_terminal()
        } else {
            let window_exists = app.get_webview_window(LABEL).is_some();
            let Some(reveal_was_pending) = state
                .readiness()
                .enter_terminal_from_loading(generation, window_exists)
            else {
                return;
            };
            reveal_was_pending
        };
        state.cancel_open_request();
        ::tracing::error!(
            event = "main_window_recovery_terminal",
            window = LABEL,
            generation,
            cause = cause.as_str(),
            failures
        );
        if state.defer_or_open_dialog(reveal_was_pending) {
            show_terminal_dialog(app);
        }
        return;
    }

    match cause {
        LoadFailureCause::VerificationTimeout | LoadFailureCause::FallbackAck => {
            let Some(replacement) = state.readiness().begin_recovery(generation, Instant::now())
            else {
                return;
            };
            start_recovery_replacement(app, generation, replacement);
        }
        LoadFailureCause::DestroyFailed => {
            let Some(retry) = state
                .readiness()
                .destroy_failed_retry(generation, Instant::now())
            else {
                return;
            };
            ::tracing::warn!(
                event = "main_window_destroy_failed",
                window = LABEL,
                generation = retry
            );
            arm_recovery_watchdog(app, retry);
            if let Some(window) = app.get_webview_window(LABEL)
                && window.destroy().is_err()
            {
                resolve_load_failure(app, retry, LoadFailureCause::DestroyFailed);
            }
        }
        LoadFailureCause::BuildFailed => {
            let Some(fresh) = state
                .readiness()
                .build_failed_retry(generation, Instant::now())
            else {
                return;
            };
            ::tracing::warn!(
                event = "main_window_build_retry",
                window = LABEL,
                generation = fresh
            );
            arm_recovery_watchdog(app, fresh);
            if let Some(window) = app.get_webview_window(LABEL) {
                if !state.readiness().defer_build_until_destroyed(fresh) {
                    return;
                }
                if window.destroy().is_err() {
                    resolve_load_failure(app, fresh, LoadFailureCause::DestroyFailed);
                }
            } else if build(app, fresh).is_err() {
                resolve_load_failure(app, fresh, LoadFailureCause::BuildFailed);
            }
        }
        LoadFailureCause::ReplacementHung => {
            let Some(fresh) = state
                .readiness()
                .replace_hung_loading(generation, Instant::now())
            else {
                return;
            };
            arm_recovery_watchdog(app, fresh);
            if let Some(window) = app.get_webview_window(LABEL) {
                if !state.readiness().defer_build_until_destroyed(fresh) {
                    return;
                }
                if window.destroy().is_err() {
                    resolve_load_failure(app, fresh, LoadFailureCause::DestroyFailed);
                }
            } else if build(app, fresh).is_err() {
                resolve_load_failure(app, fresh, LoadFailureCause::BuildFailed);
            }
        }
    }
}

fn start_recovery_replacement(app: &AppHandle, doomed: u64, replacement: u64) {
    let state = app.state::<MainWindowState>();
    ::tracing::warn!(
        event = "main_window_recovery_started",
        window = LABEL,
        generation = replacement,
        replaced_generation = doomed
    );
    flush_placement(app);
    state.invalidate_target_application(doomed);
    arm_recovery_watchdog(app, replacement);
    if let Some(window) = app.get_webview_window(LABEL) {
        if !state.readiness().defer_build_until_destroyed(replacement) {
            return;
        }
        if window.destroy().is_err() {
            resolve_load_failure(app, replacement, LoadFailureCause::DestroyFailed);
        }
    } else if build(app, replacement).is_err() {
        resolve_load_failure(app, replacement, LoadFailureCause::BuildFailed);
    }
}

fn show_terminal_dialog(app: &AppHandle) {
    let Some(state) = app.try_state::<MainWindowState>() else {
        return;
    };
    if !state.readiness().is_terminal() {
        return;
    }
    let Some(token) = state.begin_dialog() else {
        return;
    };
    let callback_app = app.clone();
    app.dialog()
        .message("The main window failed to load.")
        .title("antiburn")
        .buttons(MessageDialogButtons::OkCancelCustom(
            "Try Again".to_owned(),
            "Dismiss".to_owned(),
        ))
        .show(move |try_again| {
            on_main(&callback_app, move |app| {
                handle_dialog_result(app, token, try_again);
            });
        });
}

fn handle_dialog_result(app: &AppHandle, token: u64, try_again: bool) {
    let Some(state) = app.try_state::<MainWindowState>() else {
        return;
    };
    if !state.consume_dialog(token) {
        ::tracing::debug!(
            event = "main_window_dialog_result_stale",
            window = LABEL,
            token
        );
        return;
    }
    if !try_again {
        return;
    }
    let Some(retry) = state.readiness().retry_from_terminal(Instant::now()) else {
        ::tracing::debug!(
            event = "main_window_terminal_retry_ignored",
            window = LABEL,
            token
        );
        return;
    };
    state.reset_after_informed_retry();
    state.note_open_request(OpenTrigger::Interaction, Instant::now());
    let generation = match retry {
        TerminalRetry::RebuildAfterDestroy { generation }
        | TerminalRetry::StartLoading { generation } => generation,
    };
    ::tracing::info!(
        event = "main_window_terminal_retry",
        window = LABEL,
        generation
    );
    arm_recovery_watchdog(app, generation);
    match retry {
        TerminalRetry::RebuildAfterDestroy { generation } => {
            if !state.readiness().defer_build_until_destroyed(generation) {
                return;
            }
            if let Some(window) = app.get_webview_window(LABEL) {
                if window.destroy().is_err() {
                    resolve_load_failure(app, generation, LoadFailureCause::DestroyFailed);
                }
            } else {
                let generation = begin_terminal_retry_build(state.inner(), Instant::now());
                if let Some(generation) = generation
                    && build(app, generation).is_err()
                {
                    resolve_load_failure(app, generation, LoadFailureCause::BuildFailed);
                }
            }
        }
        TerminalRetry::StartLoading { generation } => {
            if build(app, generation).is_err() {
                resolve_load_failure(app, generation, LoadFailureCause::BuildFailed);
            }
        }
    }
}

fn begin_terminal_retry_build(state: &MainWindowState, now: Instant) -> Option<u64> {
    let mut readiness = state.readiness();
    readiness.begin_deferred_build(now)
}

/// Cancel pending reveal and verification requests, then hide the renderer.
pub fn close(window: &WebviewWindow) {
    debug_assert_main_thread();
    let state = window.app_handle().state::<MainWindowState>();
    state.readiness().cancel_pending_reveal();
    state.readiness().cancel_pending_verification();
    state.cancel_open_request();
    state.note_closed();
    flush_placement(window.app_handle());
    let _ = antiburn_main_window::conceal(window);
    emit_visibility_changed(window);
}

/// Read whether the main window can present work without treating blur as hidden.
pub fn is_visible(window: &WebviewWindow) -> bool {
    #[cfg(target_os = "macos")]
    let application_hidden = APPLICATION_HIDDEN.load(Ordering::Acquire);
    #[cfg(not(target_os = "macos"))]
    let application_hidden = false;
    visible_from_window_state(
        window.is_visible().unwrap_or(false),
        window.is_minimized().unwrap_or(true),
        application_hidden,
    )
}

const fn visible_from_window_state(
    visible: bool,
    minimized: bool,
    application_hidden: bool,
) -> bool {
    visible && !minimized && !application_hidden
}

/// Emit the current presentation visibility to the retained renderer.
pub fn emit_visibility_changed(window: &WebviewWindow) {
    if let Err(error) = window.emit(VISIBILITY_CHANGED_EVENT, is_visible(window)) {
        ::tracing::debug!(event = "main_window_visibility_emit_failed", error = %error);
    }
}

/// Return main-window presentation visibility only to the main renderer.
#[tauri::command]
pub fn get_main_window_visible(window: WebviewWindow) -> Result<bool, String> {
    if window.label() != LABEL {
        return Err("main-window visibility is unavailable to this window".to_owned());
    }
    Ok(is_visible(&window))
}

/// Restore a closed or minimized main window after the app becomes active.
#[cfg(target_os = "macos")]
pub(crate) fn restore_after_activation(app: &AppHandle) {
    let Some(state) = app.try_state::<MainWindowState>() else {
        return;
    };
    let main_visible = window_is_visible(app, LABEL);
    let main_minimized = app
        .get_webview_window(LABEL)
        .is_some_and(|window| window.is_minimized().unwrap_or(false));
    let another_window_owns_activation = crate::onboarding::is_pending(app)
        || window_is_visible(app, crate::onboarding::LABEL)
        || window_is_visible(app, crate::settings::LABEL)
        || [
            crate::popover::LABEL,
            crate::popover_peek::LABEL,
            antiburn_hud::OVERLAY_LABEL,
            antiburn_hud::DETAIL_LABEL,
            antiburn_nudge::NUDGE_LABEL,
        ]
        .into_iter()
        .any(|label| window_is_focused(app, label));
    if !state.should_restore_after_activation(
        main_visible,
        main_minimized,
        another_window_owns_activation,
    ) {
        return;
    }
    if let Err(error) = open(app, OpenTrigger::Interaction) {
        ::tracing::warn!(event = "main_window_activation_restore_failed", error = %error);
    }
}

#[cfg(target_os = "macos")]
fn window_is_visible(app: &AppHandle, label: &str) -> bool {
    app.get_webview_window(label)
        .is_some_and(|window| window.is_visible().unwrap_or(true))
}

#[cfg(target_os = "macos")]
fn window_is_focused(app: &AppHandle, label: &str) -> bool {
    app.get_webview_window(label)
        .is_some_and(|window| window.is_focused().unwrap_or(false))
}

/// Debounce a move or resize into one durable placement write.
pub fn schedule_placement_save(app: &AppHandle) {
    let generation = app
        .state::<MainWindowState>()
        .placement_generation
        .fetch_add(1, Ordering::AcqRel)
        .wrapping_add(1);
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(PLACEMENT_WRITE_DELAY).await;
        if app
            .state::<MainWindowState>()
            .placement_generation
            .load(Ordering::Acquire)
            != generation
        {
            return;
        }
        let persist_app = app.clone();
        let _ = app.run_on_main_thread(move || persist_current(&persist_app));
    });
}

/// Persist the latest normal bounds before hide or quit.
pub fn flush_placement(app: &AppHandle) {
    app.state::<MainWindowState>()
        .placement_generation
        .fetch_add(1, Ordering::AcqRel);
    persist_current(app);
}

fn persist_current(app: &AppHandle) {
    let Some(window) = app.get_webview_window(LABEL) else {
        return;
    };
    let state = app.state::<MainWindowState>();
    let previous = state.placement();
    let Some(placement) = antiburn_main_window::capture(&window, previous.as_ref()) else {
        return;
    };
    if previous.as_ref() == Some(&placement) {
        return;
    }
    let Ok(encoded) = serde_json::to_string(&placement) else {
        return;
    };
    *lock(&state.placement) = Some(placement);
    app.state::<Store>()
        .set_internal_value(PLACEMENT_KEY, &encoded);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::window_readiness::{OpenAction, ReadyAction};

    fn state() -> MainWindowState {
        MainWindowState {
            readiness: Mutex::new(WindowReadiness::default()),
            presentation: Mutex::new(Presentation::default()),
            placement: Mutex::new(None),
            placement_generation: AtomicU64::new(0),
            recovery: Mutex::new(RecoveryLedger::default()),
            renderer_status: Mutex::new(None),
            failure_reports: Mutex::new((0, 0)),
            session_target: Mutex::new(SessionTargetState::default()),
            session_target_revision: AtomicU64::new(0),
            section_target: Mutex::new(None),
            section_target_revision: AtomicU64::new(0),
            sample_targets: Mutex::new(VecDeque::new()),
        }
    }

    fn target(id: &str) -> SessionTarget {
        SessionTarget {
            agent: "codex".to_owned(),
            session_id: id.to_owned(),
            wsl_distro: None,
        }
    }

    fn ready_generation(state: &MainWindowState, now: Instant, reveal: bool) -> u64 {
        let generation = match state.readiness().request_open(now) {
            OpenAction::StartLoading { generation } => generation,
            _ => panic!("idle must load"),
        };
        if !reveal {
            state.readiness().cancel_pending_reveal();
        }
        state.readiness().renderer_ready(generation, now);
        generation
    }

    #[test]
    fn open_kind_moves_from_first_request_to_warm_reopen() {
        let state = state();
        let now = Instant::now();
        assert_eq!(
            state.note_open_request(OpenTrigger::Interaction, now),
            OpenKind::FirstOpen
        );
        assert_eq!(state.finish_reveal(now).0, OpenKind::FirstOpen);
        assert_eq!(
            state.note_open_request(OpenTrigger::Interaction, now),
            OpenKind::WarmReopen
        );
    }

    #[test]
    fn coalesced_open_keeps_the_first_request_kind_and_time() {
        let state = state();
        let first = Instant::now();
        state.note_open_request(OpenTrigger::ColdLaunch, first);
        state.note_open_request(OpenTrigger::Interaction, first + Duration::from_secs(1));
        let (kind, elapsed) = state.finish_reveal(first + Duration::from_secs(2));
        assert_eq!(kind, OpenKind::ColdLaunch);
        assert_eq!(elapsed, Duration::from_secs(2));
    }

    #[test]
    fn reveal_before_ack_retires_at_ack_time() {
        let state = state();
        let now = Instant::now();
        let generation = ready_generation(&state, now, true);
        let request = state.request_session_target(target("requested"));
        assert!(!state.retire_target_on_reveal(generation));
        assert_eq!(
            state.record_target_applied(
                generation,
                request.revision,
                Some(generation),
                Some(generation),
                true,
            ),
            TargetAck::RetiredNow
        );
        assert_eq!(
            state.peek_session_target(generation, Some(generation)),
            None
        );
    }

    #[test]
    fn ack_before_reveal_retains_then_retires() {
        let state = state();
        let now = Instant::now();
        let generation = ready_generation(&state, now, false);
        let request = state.request_session_target(target("requested"));
        assert_eq!(
            state.record_target_applied(
                generation,
                request.revision,
                Some(generation),
                Some(generation),
                false,
            ),
            TargetAck::RecordedRetained
        );
        assert!(state.retire_target_on_reveal(generation));
        assert_eq!(
            state.peek_session_target(generation, Some(generation)),
            None
        );
    }

    #[test]
    fn loading_generation_ack_is_retained_until_ready_reveal() {
        let state = state();
        let now = Instant::now();
        let generation = match state.readiness().request_open(now) {
            OpenAction::StartLoading { generation } => generation,
            _ => panic!("idle must load"),
        };
        let request = state.request_session_target(target("during-load"));

        assert_eq!(
            state
                .record_target_applied(generation, request.revision, Some(generation), None, true,),
            TargetAck::RecordedRetained
        );
        assert_eq!(
            state.peek_session_target(generation, Some(generation)),
            Some(request)
        );
        assert!(matches!(
            state.readiness().renderer_ready(generation, now),
            ReadyAction::Reveal { .. }
        ));
        assert!(state.retire_target_on_reveal(generation));
        assert_eq!(
            state.peek_session_target(generation, Some(generation)),
            None
        );
    }

    #[test]
    fn doomed_generation_ack_cannot_retire_a_retained_target() {
        let state = state();
        let now = Instant::now();
        let doomed = ready_generation(&state, now, false);
        let request = state.request_session_target(target("survives"));
        assert_eq!(
            state.record_target_applied(
                doomed,
                request.revision,
                Some(doomed),
                Some(doomed),
                false,
            ),
            TargetAck::RecordedRetained
        );
        state.invalidate_target_application(doomed);
        let replacement = state.readiness().begin_recovery(doomed, now).unwrap();
        assert_eq!(
            state.record_target_applied(doomed, request.revision, None, None, true),
            TargetAck::StaleGeneration
        );
        assert_eq!(
            state.peek_session_target(replacement, Some(replacement)),
            Some(request)
        );
    }

    #[test]
    fn a_newer_target_survives_stale_ack_and_revision_clear() {
        let state = state();
        let now = Instant::now();
        let generation = ready_generation(&state, now, true);
        let first = state.request_session_target(target("first"));
        let second = state.request_session_target(target("second"));
        assert_eq!(
            state.record_target_applied(
                generation,
                first.revision,
                Some(generation),
                Some(generation),
                true,
            ),
            TargetAck::StaleRevision
        );
        state.clear_session_target(first.revision);
        assert_eq!(
            state.peek_session_target(generation, Some(generation)),
            Some(second)
        );
    }

    #[test]
    fn accepted_recovery_state_changes_do_not_clear_the_target() {
        let state = state();
        let now = Instant::now();
        let generation = ready_generation(&state, now, false);
        let request = state.request_session_target(target("retained"));
        state.invalidate_target_application(generation);
        let replacement = state.readiness().begin_recovery(generation, now).unwrap();
        let retry = state
            .readiness()
            .build_failed_retry(replacement, now)
            .unwrap();
        assert_eq!(
            state.peek_session_target(retry, Some(retry)),
            Some(request.clone())
        );
        assert_eq!(
            state.record_target_applied(retry, request.revision, Some(retry), Some(retry), true,),
            TargetAck::RetiredNow
        );
    }

    #[test]
    fn terminal_dismiss_and_retry_retain_the_target() {
        let state = state();
        let now = Instant::now();
        let generation = match state.readiness().request_open(now) {
            OpenAction::StartLoading { generation } => generation,
            _ => panic!("idle must load"),
        };
        let request = state.request_session_target(target("terminal"));
        state
            .readiness()
            .enter_terminal_from_loading(generation, false);
        let token = state.begin_dialog().unwrap();
        assert!(state.consume_dialog(token));
        let retry = state.readiness().retry_from_terminal(now).unwrap();
        let retry_generation = match retry {
            TerminalRetry::RebuildAfterDestroy { generation }
            | TerminalRetry::StartLoading { generation } => generation,
        };
        assert_eq!(
            state.peek_session_target(retry_generation, Some(retry_generation)),
            Some(request)
        );
    }

    #[test]
    fn absent_handle_terminal_retry_releases_readiness_before_build_failure() {
        let state = state();
        let now = Instant::now();
        let generation = match state.readiness().request_open(now) {
            OpenAction::StartLoading { generation } => generation,
            _ => panic!("idle must load"),
        };
        state
            .readiness()
            .enter_terminal_from_loading(generation, true);
        let TerminalRetry::RebuildAfterDestroy { generation } =
            state.readiness().retry_from_terminal(now).unwrap()
        else {
            panic!("the terminal retry must defer its build")
        };
        assert!(state.readiness().defer_build_until_destroyed(generation));

        assert_eq!(begin_terminal_retry_build(&state, now), Some(generation));
        let readiness = state
            .readiness
            .try_lock()
            .expect("build failure handling must reacquire readiness");
        assert_eq!(readiness.loading_generation(), Some(generation));
    }

    #[test]
    fn renderer_diagnostics_reject_unknown_closed_schema_values() {
        let report = serde_json::json!({
            "kind": "unknown_kind",
            "category": "unknown",
            "errorName": "Other"
        });
        assert!(serde_json::from_value::<RenderFailureReport>(report).is_err());
    }

    #[test]
    fn healthy_status_resets_only_the_matching_active_generation() {
        let state = state();
        let now = Instant::now();
        let generation = ready_generation(&state, now, true);
        lock(&state.recovery).consecutive_failures = 2;
        assert!(!state.accept_renderer_status(generation + 1, RendererStatus::Healthy));
        assert_eq!(lock(&state.recovery).consecutive_failures, 2);
        assert!(state.accept_renderer_status(generation, RendererStatus::Fallback));
        assert_eq!(lock(&state.recovery).consecutive_failures, 2);
        assert!(state.accept_renderer_status(generation, RendererStatus::Healthy));
        assert_eq!(lock(&state.recovery).consecutive_failures, 0);
    }

    #[test]
    fn failure_reports_are_bounded_per_generation() {
        let state = state();
        let now = Instant::now();
        let generation = ready_generation(&state, now, true);
        for _ in 0..MAX_FAILURE_REPORTS_PER_GENERATION {
            assert!(state.accept_failure_report(generation));
        }
        assert!(!state.accept_failure_report(generation));
        assert!(!state.accept_failure_report(generation + 1));
    }

    #[test]
    fn hidden_terminal_dialog_is_deferred_and_each_surface_gets_a_token() {
        let state = state();
        assert!(!state.defer_or_open_dialog(false));
        assert_eq!(lock(&state.recovery).dialog, DialogState::Pending);
        let first = state.begin_dialog().unwrap();
        assert!(first > 0);
        assert_eq!(state.begin_dialog(), None);
        assert!(state.consume_dialog(first));
        let second = state.begin_dialog().unwrap();
        assert_ne!(first, second);
    }

    #[test]
    fn stale_dialog_results_change_no_recovery_state() {
        let state = state();
        lock(&state.recovery).consecutive_failures = 3;
        let token = state.begin_dialog().unwrap();
        assert!(!state.consume_dialog(token + 1));
        assert_eq!(lock(&state.recovery).consecutive_failures, 3);
        assert_eq!(lock(&state.recovery).dialog, DialogState::Open { token });
    }

    #[test]
    fn section_target_keeps_only_the_latest_request() {
        let state = state();
        state.request_section_target(MainWindowSection::BurnChecks);
        let latest = state.request_section_target(MainWindowSection::Activity);

        assert_eq!(latest.revision, 2);
        assert_eq!(state.take_section_target(), Some(latest));
        assert_eq!(state.take_section_target(), None);
    }

    #[test]
    fn sample_handles_are_opaque_bounded_and_expire() {
        let state = state();
        let now = Instant::now();
        let handle = state
            .issue_sample_handle("codex".to_owned(), "private-id".to_owned(), None, now)
            .unwrap();

        assert!(!handle.contains("private-id"));
        assert_eq!(handle.len(), 32);
        assert_eq!(
            state.resolve_sample_handle("missing", now),
            Err(SampleTargetError::Unavailable)
        );
        assert_eq!(
            state.resolve_sample_handle(&handle, now + SAMPLE_HANDLE_TTL + Duration::from_secs(1)),
            Err(SampleTargetError::Expired)
        );
    }

    #[test]
    fn sample_payload_titles_keep_stored_titles_and_use_a_neutral_fallback() {
        assert_eq!(
            sample_title(Some("Review the release")),
            "Review the release"
        );
        assert_eq!(sample_title(Some("  ")), MISSING_SAMPLE_TITLE);
        assert_eq!(sample_title(None), MISSING_SAMPLE_TITLE);
    }

    #[test]
    fn sample_payload_surfaces_are_limited_to_safe_categories() {
        assert_eq!(sample_surface("cli"), BurnCheckSampleSurface::Cli);
        assert_eq!(
            sample_surface("ide_desktop"),
            BurnCheckSampleSurface::IdeDesktop
        );
        assert_eq!(
            sample_surface("provider-private-value"),
            BurnCheckSampleSurface::Unknown
        );
    }

    #[test]
    fn sample_navigation_reports_a_deleted_session() {
        let state = state();
        let store = Store::open_in_memory(std::path::Path::new("/tmp/antiburn-sample-test"))
            .expect("open store");
        let now = Instant::now();
        let handle = state
            .issue_sample_handle("codex".to_owned(), "deleted".to_owned(), None, now)
            .unwrap();

        assert!(matches!(
            resolve_sample_for_open(&state, &store, &handle, now),
            Ok(Err(OpenBurnCheckSampleOutcome::Deleted))
        ));
    }

    #[test]
    fn a_later_external_session_request_wins_over_sample_navigation() {
        let state = state();
        let sample = state.request_session_target(SessionTarget {
            agent: "codex".to_owned(),
            session_id: "sample".to_owned(),
            wsl_distro: None,
        });
        let external = state.request_session_target(SessionTarget {
            agent: "claude-code".to_owned(),
            session_id: "external".to_owned(),
            wsl_distro: None,
        });

        assert!(external.revision > sample.revision);
        assert_eq!(lock(&state.session_target).pending, Some(external));
    }

    #[test]
    fn close_during_load_cancels_the_pending_reveal() {
        let state = state();
        let started = Instant::now();
        let generation = match state.readiness().request_open(started) {
            OpenAction::StartLoading { generation } => generation,
            _ => panic!("idle must load"),
        };
        assert!(state.readiness().cancel_pending_reveal());
        assert_eq!(
            state
                .readiness()
                .renderer_ready(generation, started + Duration::from_secs(1)),
            ReadyAction::StayHidden {
                loading_for: Duration::from_secs(1)
            }
        );
    }

    #[test]
    fn presentation_visibility_excludes_minimized_but_not_blurred_windows() {
        assert!(visible_from_window_state(true, false, false));
        assert!(!visible_from_window_state(true, true, false));
        assert!(!visible_from_window_state(false, false, false));
        assert!(!visible_from_window_state(true, false, true));
    }

    #[test]
    fn activation_restores_closed_or_minimized_main_without_other_surfaces() {
        let state = state();
        let now = Instant::now();
        assert!(!state.should_restore_after_activation(false, false, false));
        state.note_open_request(OpenTrigger::Interaction, now);
        state.finish_reveal(now);
        assert!(state.should_restore_after_activation(true, true, false));
        assert!(!state.should_restore_after_activation(true, true, true));
        state.note_closed();
        assert!(state.should_restore_after_activation(false, false, false));
    }
}
