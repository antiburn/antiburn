//! Shell policy for the ordinary main window.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use antiburn_main_window::Placement;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, WebviewWindow};

use crate::dto::{BurnCheckSamplePayload, BurnCheckSampleSurface, OpenBurnCheckSampleOutcome};
use crate::remediation::BurnCheckSampleSession;
use crate::store::{SessionKey, Store};
use crate::window_lifecycle::{self, ManagedWindowReadiness};
use crate::window_readiness::{OpenAction, WindowReadiness, renderer_generation_script};

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

/// Identity-only target for one local session.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionTarget {
    agent: String,
    session_id: String,
    wsl_distro: Option<String>,
}

/// Revisioned request shared by the event and cold-renderer take paths.
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
    // SAFETY: AppKit provides these immutable notification names on all supported macOS versions.
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
        // SAFETY: AppKit posts these notifications on the main thread. The block matches the Foundation callback signature.
        let observer = unsafe {
            center.addObserverForName_object_queue_usingBlock(Some(name), None, None, &handler)
        };
        // The observers remain registered for the app's lifetime.
        std::mem::forget(observer);
    }
}

#[cfg(not(target_os = "macos"))]
pub fn install_visibility_observers(_app: &AppHandle) {}

const PLACEMENT_KEY: &str = "internal:mainWindowPlacementV1";
const PLACEMENT_WRITE_DELAY: Duration = Duration::from_millis(350);

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

/// Renderer, presentation, and persisted placement state.
pub struct MainWindowState {
    readiness: Mutex<WindowReadiness>,
    presentation: Mutex<Presentation>,
    placement: Mutex<Option<Placement>>,
    placement_generation: AtomicU64,
    session_target: Mutex<Option<SessionTargetRequest>>,
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
            session_target: Mutex::new(None),
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
            revision: self
                .session_target_revision
                .fetch_add(1, Ordering::AcqRel)
                .wrapping_add(1),
            target,
        };
        *lock(&self.session_target) = Some(request.clone());
        request
    }

    fn take_session_target(&self) -> Option<SessionTargetRequest> {
        lock(&self.session_target).take()
    }

    fn clear_session_target(&self, revision: u64) {
        let mut target = lock(&self.session_target);
        if target
            .as_ref()
            .is_some_and(|request| request.revision == revision)
        {
            *target = None;
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

/// Open the main window or coalesce with its active renderer load.
pub fn open(app: &AppHandle, trigger: OpenTrigger) -> tauri::Result<()> {
    let now = Instant::now();
    let state = app.state::<MainWindowState>();
    let open_kind = state.note_open_request(trigger, now);
    ::tracing::info!(
        event = "main_window_open_requested",
        window = LABEL,
        open_kind = open_kind.as_str()
    );

    let action = state.readiness().request_open(now);
    match action {
        OpenAction::Reveal => reveal(app.get_webview_window(LABEL).as_ref()),
        OpenAction::AwaitReady => Ok(()),
        OpenAction::StartLoading { generation } => build(app, generation),
        OpenAction::Rebuild { generation } => {
            let Some(window) = app.get_webview_window(LABEL) else {
                return build(app, generation);
            };
            if !state.readiness().defer_build_until_destroyed(generation) {
                return Ok(());
            }
            if let Err(error) = window.destroy() {
                window_lifecycle::cancel_load::<MainWindowState>(app, generation);
                state.cancel_open_request();
                return Err(error);
            }
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

/// Take the latest session target after the main renderer installs its listener.
#[tauri::command]
pub fn take_main_window_session_target(
    window: WebviewWindow,
) -> Result<Option<SessionTargetRequest>, String> {
    if window.label() != LABEL {
        return Err("main-window session targets are unavailable to this window".to_owned());
    }
    Ok(window.state::<MainWindowState>().take_session_target())
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
    match antiburn_main_window::build(
        app,
        renderer_generation_script(generation),
        placement.as_ref(),
        |window, payload| {
            window_lifecycle::trace_page_load::<MainWindowState>(window, payload, LABEL);
        },
    ) {
        Ok(built) => {
            let applied = built
                .placement
                .or_else(|| antiburn_main_window::capture(&built.window, None));
            state.remember_placement(applied);
            Ok(())
        }
        Err(error) => {
            window_lifecycle::cancel_load::<MainWindowState>(app, generation);
            state.cancel_open_request();
            Err(error)
        }
    }
}

/// Build a deferred stale-load replacement after its old label is removed.
pub fn rebuild_after_destroy(app: &AppHandle) {
    let generation = window_lifecycle::begin_deferred_build::<MainWindowState>(app, Instant::now());
    let Some(generation) = generation else {
        return;
    };
    if let Err(error) = build(app, generation) {
        ::tracing::error!(event = "window_rebuild_failed", window = LABEL, error = %error);
    }
}

/// Accept readiness only from the main renderer's active generation.
pub fn renderer_ready(window: &WebviewWindow, generation: u64) {
    let app = window.app_handle();
    if !window_lifecycle::renderer_ready::<MainWindowState>(app, LABEL, generation, Instant::now())
    {
        return;
    }
    let (open_kind, elapsed) = app
        .state::<MainWindowState>()
        .request_details(Instant::now());
    ::tracing::info!(
        event = "main_window_renderer_ready",
        window = LABEL,
        generation,
        open_kind = open_kind.as_str(),
        elapsed_ms = elapsed.as_millis() as u64
    );
    if let Err(error) = reveal(Some(window)) {
        ::tracing::error!(event = "window_reveal_failed", window = LABEL, error = %error);
    }
}

fn reveal(window: Option<&WebviewWindow>) -> tauri::Result<()> {
    let Some(window) = window else {
        return Ok(());
    };
    antiburn_main_window::reveal(window)?;
    let (open_kind, elapsed) = window
        .app_handle()
        .state::<MainWindowState>()
        .finish_reveal(Instant::now());
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

/// Cancel a pending reveal and hide the renderer.
pub fn close(window: &WebviewWindow) {
    let state = window.app_handle().state::<MainWindowState>();
    state.readiness().cancel_pending_reveal();
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
    use crate::window_readiness::ReadyAction;

    #[test]
    fn open_kind_moves_from_first_request_to_warm_reopen() {
        let state = MainWindowState {
            readiness: Mutex::new(WindowReadiness::default()),
            presentation: Mutex::new(Presentation::default()),
            placement: Mutex::new(None),
            placement_generation: AtomicU64::new(0),
            session_target: Mutex::new(None),
            session_target_revision: AtomicU64::new(0),
            section_target: Mutex::new(None),
            section_target_revision: AtomicU64::new(0),
            sample_targets: Mutex::new(VecDeque::new()),
        };
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
        let state = MainWindowState {
            readiness: Mutex::new(WindowReadiness::default()),
            presentation: Mutex::new(Presentation::default()),
            placement: Mutex::new(None),
            placement_generation: AtomicU64::new(0),
            session_target: Mutex::new(None),
            session_target_revision: AtomicU64::new(0),
            section_target: Mutex::new(None),
            section_target_revision: AtomicU64::new(0),
            sample_targets: Mutex::new(VecDeque::new()),
        };
        let first = Instant::now();
        assert_eq!(
            state.note_open_request(OpenTrigger::ColdLaunch, first),
            OpenKind::ColdLaunch
        );
        assert_eq!(
            state.note_open_request(OpenTrigger::Interaction, first + Duration::from_secs(1)),
            OpenKind::ColdLaunch
        );
        let (kind, elapsed) = state.finish_reveal(first + Duration::from_secs(2));
        assert_eq!(kind, OpenKind::ColdLaunch);
        assert_eq!(elapsed, Duration::from_secs(2));
    }

    #[test]
    fn session_target_keeps_only_the_latest_revision_and_wsl_identity() {
        let state = MainWindowState {
            readiness: Mutex::new(WindowReadiness::default()),
            presentation: Mutex::new(Presentation::default()),
            placement: Mutex::new(None),
            placement_generation: AtomicU64::new(0),
            session_target: Mutex::new(None),
            session_target_revision: AtomicU64::new(0),
            section_target: Mutex::new(None),
            section_target_revision: AtomicU64::new(0),
            sample_targets: Mutex::new(VecDeque::new()),
        };
        state.request_session_target(SessionTarget {
            agent: "claude".to_owned(),
            session_id: "old".to_owned(),
            wsl_distro: None,
        });
        let latest = state.request_session_target(SessionTarget {
            agent: "codex".to_owned(),
            session_id: "new".to_owned(),
            wsl_distro: Some("Ubuntu-24.04".to_owned()),
        });

        assert_eq!(latest.revision, 2);
        assert_eq!(state.take_session_target(), Some(latest));
        assert_eq!(state.take_session_target(), None);
    }

    #[test]
    fn session_target_clear_does_not_remove_a_newer_request() {
        let state = MainWindowState {
            readiness: Mutex::new(WindowReadiness::default()),
            presentation: Mutex::new(Presentation::default()),
            placement: Mutex::new(None),
            placement_generation: AtomicU64::new(0),
            session_target: Mutex::new(None),
            session_target_revision: AtomicU64::new(0),
            section_target: Mutex::new(None),
            section_target_revision: AtomicU64::new(0),
            sample_targets: Mutex::new(VecDeque::new()),
        };
        let first = state.request_session_target(SessionTarget {
            agent: "claude".to_owned(),
            session_id: "first".to_owned(),
            wsl_distro: None,
        });
        let second = state.request_session_target(SessionTarget {
            agent: "codex".to_owned(),
            session_id: "second".to_owned(),
            wsl_distro: None,
        });

        state.clear_session_target(first.revision);
        assert_eq!(state.take_session_target(), Some(second));
    }

    #[test]
    fn section_target_keeps_only_the_latest_request() {
        let state = MainWindowState {
            readiness: Mutex::new(WindowReadiness::default()),
            presentation: Mutex::new(Presentation::default()),
            placement: Mutex::new(None),
            placement_generation: AtomicU64::new(0),
            session_target: Mutex::new(None),
            session_target_revision: AtomicU64::new(0),
            section_target: Mutex::new(None),
            section_target_revision: AtomicU64::new(0),
            sample_targets: Mutex::new(VecDeque::new()),
        };
        state.request_section_target(MainWindowSection::BurnChecks);
        let latest = state.request_section_target(MainWindowSection::Activity);

        assert_eq!(latest.revision, 2);
        assert_eq!(state.take_section_target(), Some(latest));
        assert_eq!(state.take_section_target(), None);
    }

    #[test]
    fn sample_handles_are_opaque_bounded_and_expire() {
        let state = MainWindowState {
            readiness: Mutex::new(WindowReadiness::default()),
            presentation: Mutex::new(Presentation::default()),
            placement: Mutex::new(None),
            placement_generation: AtomicU64::new(0),
            session_target: Mutex::new(None),
            session_target_revision: AtomicU64::new(0),
            section_target: Mutex::new(None),
            section_target_revision: AtomicU64::new(0),
            sample_targets: Mutex::new(VecDeque::new()),
        };
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
        let state = MainWindowState {
            readiness: Mutex::new(WindowReadiness::default()),
            presentation: Mutex::new(Presentation::default()),
            placement: Mutex::new(None),
            placement_generation: AtomicU64::new(0),
            session_target: Mutex::new(None),
            session_target_revision: AtomicU64::new(0),
            section_target: Mutex::new(None),
            section_target_revision: AtomicU64::new(0),
            sample_targets: Mutex::new(VecDeque::new()),
        };
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
        let state = MainWindowState {
            readiness: Mutex::new(WindowReadiness::default()),
            presentation: Mutex::new(Presentation::default()),
            placement: Mutex::new(None),
            placement_generation: AtomicU64::new(0),
            session_target: Mutex::new(None),
            session_target_revision: AtomicU64::new(0),
            section_target: Mutex::new(None),
            section_target_revision: AtomicU64::new(0),
            sample_targets: Mutex::new(VecDeque::new()),
        };
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
        assert_eq!(state.take_session_target(), Some(external));
    }

    #[test]
    fn close_during_load_cancels_the_pending_reveal() {
        let state = MainWindowState {
            readiness: Mutex::new(WindowReadiness::default()),
            presentation: Mutex::new(Presentation::default()),
            placement: Mutex::new(None),
            placement_generation: AtomicU64::new(0),
            session_target: Mutex::new(None),
            session_target_revision: AtomicU64::new(0),
            section_target: Mutex::new(None),
            section_target_revision: AtomicU64::new(0),
            sample_targets: Mutex::new(VecDeque::new()),
        };
        let started = Instant::now();
        let OpenAction::StartLoading { generation } = state.readiness().request_open(started)
        else {
            panic!("an idle main window starts loading")
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
    fn activation_restores_closed_or_minimized_main_without_interrupting_other_surfaces() {
        let state = MainWindowState {
            readiness: Mutex::new(WindowReadiness::default()),
            presentation: Mutex::new(Presentation::default()),
            placement: Mutex::new(None),
            placement_generation: AtomicU64::new(0),
            session_target: Mutex::new(None),
            session_target_revision: AtomicU64::new(0),
            section_target: Mutex::new(None),
            section_target_revision: AtomicU64::new(0),
            sample_targets: Mutex::new(VecDeque::new()),
        };
        let now = Instant::now();

        assert!(!state.should_restore_after_activation(false, false, false));
        assert!(!state.should_restore_after_activation(false, true, false));
        state.note_open_request(OpenTrigger::Interaction, now);
        state.finish_reveal(now);
        assert!(state.should_restore_after_activation(true, true, false));
        assert!(state.should_restore_after_activation(false, true, false));
        assert!(!state.should_restore_after_activation(true, true, true));
        assert!(!state.should_restore_after_activation(true, false, false));
        state.note_closed();

        assert!(lock(&state.presentation).pending.is_none());
        assert!(!state.should_restore_after_activation(true, false, false));
        assert!(!state.should_restore_after_activation(false, false, true));
        assert!(state.should_restore_after_activation(false, false, false));

        state.note_open_request(OpenTrigger::Interaction, now);
        state.finish_reveal(now);
        assert!(!state.should_restore_after_activation(false, false, false));
    }
}
