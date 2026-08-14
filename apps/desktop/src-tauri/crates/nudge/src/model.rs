// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The nudge content model — the wire contract between the app (which builds
//! payloads) and the notification webview (which renders them).
//!
//! The renderer is intentionally **kind-agnostic**: it draws whatever fields are
//! present (title, optional reason, optional recommendations, N actions). Adding
//! a new trigger is a new [`NudgeKind`] variant plus an app-side payload builder
//! and action route — no change to this model's shape and no change to the
//! notification UI. `kind` is `#[non_exhaustive]` so adding variants is not a
//! breaking change.
//!
//! Everything here serializes `camelCase`, which is what the rest of the shell's
//! IPC surface speaks (see `src-tauri/src/dto.rs`) — so a nudge payload reads
//! the same way in the webview as every other payload it receives.

use serde::{Deserialize, Serialize};

/// What surfaced the nudge. Extend as new triggers are added.
///
/// `Ord` is derived only so kinds can key a map — the app parks deferred nudges
/// one slot per kind. The ordering itself is declaration order and carries no
/// meaning; nothing may treat it as a priority.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub enum NudgeKind {
    /// An update check found a newer version.
    UpdateAvailable,
    /// A scan could not finish.
    ScanFailure,
    /// Free space on the startup volume fell below the user's threshold.
    DiskSpaceLow,
    /// Sustained spend is unusually fast for this machine.
    UsageAnomaly,
    /// A provider quota window crossed a configured usage milestone.
    UsageMilestone,
    /// User-triggered preview from notification settings.
    Test,
}

/// Visual tone — selects the notification's accent color and icon. Maps cleanly
/// onto OS-notification conventions (informational / positive / attention).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NudgeTone {
    Info,
    Success,
    Warning,
}

/// A single actionable CTA on the notification.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NudgeAction {
    /// Stable identifier routed back to the app on click.
    pub id: String,
    /// Button label.
    pub label: String,
    /// Render as the emphasized/primary button.
    #[serde(default)]
    pub primary: bool,
    /// Optional structured target metadata for the action.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<NudgeActionTarget>,
}

/// Optional target metadata attached to an action and echoed back to the app.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum NudgeActionTarget {
    #[serde(rename_all = "camelCase")]
    ProviderUsage {
        provider: String,
        account_key: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    Session {
        agent: String,
        session_id: String,
        environment: Option<String>,
    },
}

/// A nudge payload: a title, an optional reason, and optional recommendations,
/// plus zero or more actions. Serialized to the notification webview as the
/// `nudge:show` event body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Nudge {
    /// Unique instance id (for de-dupe).
    pub id: String,
    pub kind: NudgeKind,
    pub tone: NudgeTone,
    pub title: String,
    /// The person or agent this is about, when it is about one. `None` when it
    /// is about nobody — a disk-space warning is about a volume, not somebody's
    /// doing.
    ///
    /// Not drawn by the renderer: builders normally work the name into the title
    /// and this carries it separately, so the app can still act on *who* after
    /// the title has become a sentence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor: Option<String>,
    /// Why this is being shown (a short description). Rendered only when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Optional suggested steps, rendered as a tight list once expanded.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub recommendations: Vec<String>,
    /// Actionable CTAs.
    pub actions: Vec<NudgeAction>,
    /// Auto-dismiss timeout in milliseconds; `None` = sticky until acted on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u32>,
}

impl Nudge {
    /// Start a nudge with the required fields; chain the optional builders below.
    pub fn new(
        id: impl Into<String>,
        kind: NudgeKind,
        tone: NudgeTone,
        title: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            kind,
            tone,
            title: title.into(),
            actor: None,
            reason: None,
            recommendations: Vec::new(),
            actions: Vec::new(),
            timeout_ms: None,
        }
    }

    pub fn reason(mut self, reason: impl Into<String>) -> Self {
        self.reason = Some(reason.into());
        self
    }

    /// Name whoever this is about. A blank or whitespace-only name is treated as
    /// nobody, so a payload with an empty author field can't be mistaken for a
    /// person named "".
    pub fn actor(mut self, actor: Option<impl Into<String>>) -> Self {
        self.actor = actor.map(Into::into).filter(|name| !name.trim().is_empty());
        self
    }

    pub fn recommendations(mut self, recommendations: Vec<String>) -> Self {
        self.recommendations = recommendations;
        self
    }

    /// Append a CTA. Chainable; call order doesn't matter — `push_action` keeps
    /// the primary action last regardless.
    pub fn action(
        mut self,
        id: impl Into<String>,
        label: impl Into<String>,
        primary: bool,
    ) -> Self {
        self.push_action(NudgeAction {
            id: id.into(),
            label: label.into(),
            primary,
            target: None,
        });
        self
    }

    /// Append a CTA with structured target metadata. Same ordering guarantee as
    /// [`Nudge::action`].
    pub fn action_with_target(
        mut self,
        id: impl Into<String>,
        label: impl Into<String>,
        primary: bool,
        target: NudgeActionTarget,
    ) -> Self {
        self.push_action(NudgeAction {
            id: id.into(),
            label: label.into(),
            primary,
            target: Some(target),
        });
        self
    }

    /// Push an action and re-sort so the primary action (if any) is always
    /// last. `NudgeView.tsx` renders `actions` in array order with no reversal,
    /// so this is what guarantees the default button always renders rightmost
    /// (macOS convention) — independent of the order `build_xxx()` functions
    /// call `.action()`/`.action_with_target()` in. `sort_by_key` is stable, so
    /// non-primary actions keep their relative call order.
    fn push_action(&mut self, action: NudgeAction) {
        self.actions.push(action);
        self.actions.sort_by_key(|a| a.primary);
    }

    pub fn timeout_ms(mut self, ms: u32) -> Self {
        self.timeout_ms = Some(ms);
        self
    }
}

/// Reported back to the app when a CTA is clicked. The app maps `(kind,
/// action_id)` to a navigation; the crate never interprets it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NudgeActionEvent {
    pub kind: NudgeKind,
    pub action_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<NudgeActionTarget>,
}
