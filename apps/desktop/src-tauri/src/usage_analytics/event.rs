// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The event payload: every field that may ever leave this machine, named once.
//!
//! This module is the enforcement point for the promise the Privacy pane makes.
//! [`Event`] has no free-form field — no map, no `serde_json::Value`, no
//! `String` a caller chooses the contents of — so there is nowhere for a path,
//! a repository name, a session title, or a credential to be put. Adding a
//! field here is the only way to widen what is sent, which makes widening it a
//! visible act in review rather than an accident at a call site.
//!
//! Counts are bucketed for the same reason. An exact session count, reported
//! repeatedly over weeks, is a fingerprint even without an identifier attached
//! to it; a bucket is the answer to "roughly how much is this being used"
//! without also answering "is this the same machine as last week".

use antiburn_local::model::AgentKind;
use serde::{Deserialize, Serialize};

/// The events this application may report. A closed set, by design: a
/// `&'static str` a caller passes in would put naming — and therefore scope —
/// back at the call site.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventName {
    /// The application started.
    AppLaunched,
    /// The first run finished, or was abandoned at a named step.
    OnboardingFinished,
    /// A discovery pass completed, with a bucketed count of what it found.
    ScanCompleted,
    /// A preference changed. The key travels; the value never does.
    SettingToggled,
    /// A session was opened from the activity list.
    SessionOpened,
    /// The usage view was opened.
    UsageViewed,
    /// Something failed, by category. No message, no path, no backtrace.
    ErrorOccurred,
}

/// Every event this application may send.
///
/// Test-only, because nothing in the running app iterates the catalog — but it
/// is the list the documentation test walks, so a variant missing from here
/// would make that test pass vacuously. `no_variant_escapes_the_catalog` below
/// closes that with an exhaustive match: adding a variant is a *compile* error
/// until it is listed here, and listing it here is what forces it into
/// `docs/usage-analytics.md`.
#[cfg(test)]
pub const EVERY_EVENT: &[EventName] = &[
    EventName::AppLaunched,
    EventName::OnboardingFinished,
    EventName::ScanCompleted,
    EventName::SettingToggled,
    EventName::SessionOpened,
    EventName::UsageViewed,
    EventName::ErrorOccurred,
];

impl EventName {
    pub fn as_str(self) -> &'static str {
        match self {
            EventName::AppLaunched => "antiburn.app_launched",
            EventName::OnboardingFinished => "antiburn.onboarding_finished",
            EventName::ScanCompleted => "antiburn.scan_completed",
            EventName::SettingToggled => "antiburn.setting_toggled",
            EventName::SessionOpened => "antiburn.session_opened",
            EventName::UsageViewed => "antiburn.usage_viewed",
            EventName::ErrorOccurred => "antiburn.error_occurred",
        }
    }
}

/// One event, in the collector's wire envelope.
///
/// The shape is the first-party collector's documented `POST /v1/track`
/// contract, implemented from that contract rather than ported from its
/// client — no source crossed into this repository, and the fields below are
/// the whole of what antiburn fills in. Two halves of that contract are
/// deliberately *not* implemented: there is no `identify` call and no
/// `userId`/`orgId`, because antiburn has no account, no organisation, and
/// nothing to stitch a pre-login history to.
///
/// `sentAt` is absent here on purpose. It is stamped at delivery, not at
/// capture, so the server can correct for clock skew against
/// `originalTimestamp`; a queued event that waited an hour must not claim it
/// was sent when it was recorded.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Event {
    /// Always `desktop`, the surface class the collector partitions on.
    pub platform: &'static str,
    /// Per-event UUID. The collector's deduplication key, which is what makes
    /// re-delivering a batch after a failed flush safe.
    pub message_id: String,
    /// The rotating installation identifier. Random, not derived from any
    /// machine fact, and replaced every [`super::IDENTITY_LIFETIME_DAYS`].
    ///
    /// The contract's own client keeps this stable and shares it with device
    /// telemetry so rows join across surfaces. antiburn rotates it instead:
    /// joining a history is the thing the register's D-28 promises it cannot
    /// do, and that promise is worth more here than the join.
    pub anonymous_id: String,
    /// The run identifier the contract requires on every track call.
    ///
    /// Required by the collector, not optional: a payload without it is
    /// rejected outright, so this is the contract's floor rather than
    /// something antiburn chose to add. It is also the *least* persistent
    /// thing in the payload — minted in memory, never written to disk, gone
    /// when the process exits, and replaced after
    /// [`super::SESSION_TIMEOUT`] of inactivity. It cannot outlive a run, so
    /// it cannot join one to another; the rotating [`Event::anonymous_id`]
    /// remains the longest-lived identifier here, which is the property
    /// D-28 turns on.
    pub session_id: String,
    /// Event name, in antiburn's own namespace.
    pub event: String,
    /// When it happened, RFC 3339 UTC.
    pub original_timestamp: String,
    /// The closed property set. Not a free-form map: see the module docs.
    pub properties: Properties,
    /// Install context, the two fields the contract stamps on every track.
    pub context: Context,
}

/// Everything an event may carry beyond its name. A struct rather than a JSON
/// object so there is nowhere for a path or a title to be put.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Properties {
    /// CPU architecture.
    pub arch: &'static str,
    /// A bucketed magnitude, where the event has one. Never an exact count.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bucket: Option<&'static str>,
    /// A bare key or category from a closed vocabulary, never reader text.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<&'static str>,
    /// A second dimension, where one event has two things worth separating —
    /// which agent's session was opened *and* whether it ran natively or
    /// under WSL. Same rules as [`Properties::label`], and the same reason
    /// for the `&'static str`: only a compile-time constant can be put here,
    /// so no amount of caller error can turn it into a path or a title.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<&'static str>,
}

/// What a caller may attach to an event.
///
/// A struct rather than three positional `Option<&'static str>` arguments,
/// which are indistinguishable to the compiler and so silently swappable at a
/// call site. Naming them makes a mix-up a compile error instead of a wrong
/// value arriving in a dashboard nobody cross-checks.
#[derive(Debug, Clone, Copy, Default)]
pub struct Facts {
    /// A bucketed magnitude, never an exact count.
    pub bucket: Option<&'static str>,
    /// The primary dimension.
    pub label: Option<&'static str>,
    /// The secondary dimension, where the event has one.
    pub detail: Option<&'static str>,
}

/// The install context the contract stamps on every track event.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Context {
    /// The canonical `antiburn:<version>` string, the same form the
    /// contract's `desktop:<version>` takes for its own surface.
    pub app_version: String,
    /// Operating-system family — `macos`, `windows`, `linux`.
    pub os: &'static str,
}

/// An interaction the renderer may report.
///
/// This is the enforcement point for events fired from the webview, and it is
/// why there is no general "record an event" command. A command taking a name
/// and a map would move naming — and therefore scope — into TypeScript, where
/// nothing stops a future call site from passing a repository name. Here the
/// renderer may only name a shape that already exists, serde rejects anything
/// outside it, and every string that reaches the payload is a `&'static str`
/// this file wrote.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Interaction {
    /// A session was opened from the activity list. `agent` deserializes into
    /// the engine's own closed enum, so an unrecognised slug is a rejected
    /// command rather than a new value appearing in the data.
    SessionOpened {
        agent: AgentKind,
        environment: Environment,
    },
    /// The usage view was opened, with how much there was to show.
    UsageViewed {
        providers: u64,
        evidence: UsageEvidence,
    },
}

/// Where an agent ran. Two values, and neither names anything: a WSL
/// distribution's *name* is chosen by the reader and is deliberately not
/// carried, unlike the contract's own client, which sends it.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Environment {
    Native,
    Wsl,
}

/// What the usage view had to work with when it opened.
///
/// The product question this exists to answer is whether an installation ever
/// gets the provider's own figures or only antiburn's estimates from local
/// transcripts. Three values answer it; a per-provider breakdown would not
/// answer it better and would say more about the reader.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UsageEvidence {
    /// At least one provider reported its own limit figures.
    Live,
    /// Estimates from local transcripts only.
    EstimatedOnly,
    /// Nothing to show.
    None,
}

impl Interaction {
    /// The event and the facts this interaction becomes.
    pub fn resolve(self) -> (EventName, Facts) {
        match self {
            Interaction::SessionOpened { agent, environment } => (
                EventName::SessionOpened,
                Facts {
                    label: Some(agent.slug()),
                    detail: Some(environment.as_str()),
                    ..Facts::default()
                },
            ),
            Interaction::UsageViewed {
                providers,
                evidence,
            } => (
                EventName::UsageViewed,
                Facts {
                    bucket: Some(bucket(providers)),
                    label: Some(evidence.as_str()),
                    ..Facts::default()
                },
            ),
        }
    }
}

impl Environment {
    fn as_str(self) -> &'static str {
        match self {
            Environment::Native => "native",
            Environment::Wsl => "wsl",
        }
    }
}

impl UsageEvidence {
    fn as_str(self) -> &'static str {
        match self {
            UsageEvidence::Live => "live",
            UsageEvidence::EstimatedOnly => "estimated_only",
            UsageEvidence::None => "none",
        }
    }
}

/// Collapse a count into the bucket that ships in its place.
pub fn bucket(count: u64) -> &'static str {
    match count {
        0 => "0",
        1..=9 => "1-9",
        10..=49 => "10-49",
        50..=199 => "50-199",
        200..=999 => "200-999",
        _ => "1000+",
    }
}

/// The surface class the collector partitions on. antiburn is a desktop
/// application, and the contract's vocabulary has one value for that.
pub const PLATFORM: &str = "desktop";

/// The operating-system family, as the payload reports it.
pub fn os_family() -> &'static str {
    std::env::consts::OS
}

/// The CPU architecture, as the payload reports it.
pub fn arch() -> &'static str {
    std::env::consts::ARCH
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Event {
        Event {
            platform: PLATFORM,
            message_id: "11111111-1111-4111-8111-111111111111".into(),
            anonymous_id: "22222222-2222-4222-8222-222222222222".into(),
            session_id: "33333333-3333-4333-8333-333333333333".into(),
            event: EventName::ScanCompleted.as_str().into(),
            original_timestamp: "2026-08-18T00:00:00Z".into(),
            properties: Properties {
                arch: "aarch64",
                bucket: Some("10-49"),
                label: Some("claude-code"),
                detail: Some("native"),
            },
            context: Context {
                app_version: "antiburn:1.2.3".into(),
                os: "macos",
            },
        }
    }

    #[test]
    fn buckets_never_reveal_an_exact_count() {
        assert_eq!(bucket(0), "0");
        assert_eq!(bucket(1), "1-9");
        assert_eq!(bucket(9), "1-9");
        assert_eq!(bucket(10), "10-49");
        assert_eq!(bucket(4_000), "1000+");
    }

    /// The complete wire surface, pinned.
    ///
    /// This test cannot read the Privacy pane, so it does not pretend to: it
    /// pins the exact set of keys that go out, and the pane's "Exactly what is
    /// sent" row has to be updated in the same change as any diff here. An
    /// earlier version of this test was named as though it checked the copy,
    /// which is worse than not checking it — the name asserted a guarantee
    /// nothing enforced, and five fields went unnamed in the pane for exactly
    /// that reason. Adding a field below without touching
    /// `apps/desktop/src/views/settings/PrivacyPane.tsx` is the bug this
    /// comment exists to prevent.
    #[test]
    fn the_wire_payload_is_exactly_these_thirteen_fields() {
        let json = serde_json::to_value(sample()).expect("serializes");
        let object = json.as_object().expect("an object");
        let mut keys: Vec<_> = object.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            [
                "anonymousId",
                "context",
                "event",
                "messageId",
                "originalTimestamp",
                "platform",
                "properties",
                "sessionId",
            ]
        );

        let mut property_keys: Vec<_> = object["properties"]
            .as_object()
            .expect("properties object")
            .keys()
            .map(String::as_str)
            .collect();
        property_keys.sort_unstable();
        assert_eq!(property_keys, ["arch", "bucket", "detail", "label"]);

        let mut context_keys: Vec<_> = object["context"]
            .as_object()
            .expect("context object")
            .keys()
            .map(String::as_str)
            .collect();
        context_keys.sort_unstable();
        assert_eq!(context_keys, ["appVersion", "os"]);
    }

    /// `sentAt` belongs to delivery, not to capture. A queued event that
    /// waited an hour must not claim it was sent when it was recorded, so the
    /// stamp is spliced in at the moment of the request instead.
    #[test]
    fn a_captured_event_carries_no_sent_at() {
        let json = serde_json::to_string(&sample()).expect("serializes");
        assert!(!json.contains("sentAt"), "{json}");
    }

    /// Nothing about the reader's account, because there is no account. The
    /// contract's identify half is deliberately unimplemented.
    #[test]
    fn no_user_or_organisation_identity_is_ever_carried() {
        let json = serde_json::to_string(&sample()).expect("serializes");
        // `sessionId` is deliberately absent from this list: it identifies a
        // run of the process, not a person, and never reaches disk.
        for forbidden in ["userId", "orgId", "email", "locale"] {
            assert!(!json.contains(forbidden), "{forbidden} in {json}");
        }
    }

    #[test]
    fn absent_optional_fields_are_omitted_rather_than_null() {
        let mut event = sample();
        event.properties.bucket = None;
        event.properties.label = None;
        event.properties.detail = None;
        let json = serde_json::to_string(&event).expect("serializes");
        assert!(!json.contains("bucket"), "{json}");
        assert!(!json.contains("label"), "{json}");
        assert!(!json.contains("detail"), "{json}");
    }

    /// The compiler, not a reviewer, keeps [`EVERY_EVENT`] complete.
    ///
    /// A new variant makes this match non-exhaustive, which fails the build at
    /// this line — the one place that then points at the catalog and at the
    /// document the event has to appear in.
    #[test]
    fn no_variant_escapes_the_catalog() {
        fn listed(event: EventName) -> bool {
            match event {
                EventName::AppLaunched
                | EventName::OnboardingFinished
                | EventName::ScanCompleted
                | EventName::SettingToggled
                | EventName::SessionOpened
                | EventName::UsageViewed
                | EventName::ErrorOccurred => true,
            }
        }
        assert_eq!(
            EVERY_EVENT.len(),
            7,
            "a variant was added to the match above but not to EVERY_EVENT"
        );
        assert!(EVERY_EVENT.iter().copied().all(listed));
    }

    /// The public catalog in `docs/usage-analytics.md` is a promise to a
    /// reader who cannot read this file, so it cannot be allowed to drift.
    /// Adding a variant above without documenting it fails here rather than
    /// shipping a document that quietly under-reports what is sent.
    #[test]
    fn the_documented_catalog_matches_the_code() {
        let doc = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../docs/usage-analytics.md"
        ));
        for name in EVERY_EVENT {
            assert!(
                doc.contains(name.as_str()),
                "{} is sent but is not in docs/usage-analytics.md",
                name.as_str()
            );
        }
    }

    /// An interaction resolves to constants this file owns, never to
    /// anything the renderer supplied verbatim.
    #[test]
    fn an_interaction_resolves_to_this_files_own_constants() {
        let (name, facts) = Interaction::SessionOpened {
            agent: AgentKind::Claude,
            environment: Environment::Wsl,
        }
        .resolve();
        assert_eq!(name, EventName::SessionOpened);
        assert_eq!(facts.label, Some("claude-code"));
        assert_eq!(facts.detail, Some("wsl"));
        assert_eq!(facts.bucket, None);

        let (name, facts) = Interaction::UsageViewed {
            providers: 12,
            evidence: UsageEvidence::EstimatedOnly,
        }
        .resolve();
        assert_eq!(name, EventName::UsageViewed);
        assert_eq!(facts.bucket, Some("10-49"), "counts are never exact");
        assert_eq!(facts.label, Some("estimated_only"));
        assert_eq!(facts.detail, None);
    }

    /// The renderer cannot invent a value. This is the whole reason the IPC
    /// edge takes a closed enum rather than a name and a map: a call site in
    /// TypeScript that passed a repository name would not compile into
    /// anything the collector could receive — it fails at the command
    /// boundary instead.
    #[test]
    fn an_unrecognised_interaction_is_refused_at_the_boundary() {
        let unknown_agent = serde_json::json!({
            "kind": "sessionOpened",
            "agent": "some-repository-name",
            "environment": "native",
        });
        assert!(serde_json::from_value::<Interaction>(unknown_agent).is_err());

        let unknown_shape = serde_json::json!({
            "kind": "somethingElse",
            "path": "/Users/someone/work",
        });
        assert!(serde_json::from_value::<Interaction>(unknown_shape).is_err());

        let unknown_environment = serde_json::json!({
            "kind": "sessionOpened",
            "agent": "claude-code",
            "environment": "Ubuntu-24.04",
        });
        assert!(serde_json::from_value::<Interaction>(unknown_environment).is_err());
    }

    #[test]
    fn every_event_name_sits_in_antiburns_own_namespace() {
        for name in EVERY_EVENT {
            assert!(name.as_str().starts_with("antiburn."), "{}", name.as_str());
        }
    }
}
