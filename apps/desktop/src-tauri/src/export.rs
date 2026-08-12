// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The session export document.
//!
//! An export is a **derived** record: session identity, the engine's analysis,
//! the cost estimate, the skills it invoked, and its sub-agent roster. The
//! transcript itself is never copied — the document carries a reference to
//! where it lives on this machine instead, so a reader can find it without the
//! export becoming a second copy of their conversation.
//!
//! That is still not the same as "safe to publish". Titles, working directory
//! paths, repository names, model ids, and skill names all describe real work,
//! which is exactly why the export flow warns before it writes.

use antiburn_local::analysis::{SessionCost, SessionMetrics, SkillUse};
use serde::Serialize;

use crate::dto::OrchestrationStatus;

/// Format marker, so a reader (or a future importer) can recognize the file.
pub const FORMAT: &str = "antiburn.session-export";

/// Bumped whenever the document's shape changes incompatibly.
pub const FORMAT_VERSION: u32 = 1;

/// Stated in the document itself, so the contract travels with the file.
const CONTENT_NOTICE: &str = concat!(
    "This export contains derived analysis only. No transcript content, ",
    "message text, tool arguments, or file contents are included. ",
    "`session.sourcePath` references the provider's own transcript on the ",
    "machine this was exported from; it is not copied here."
);

/// Who and where the exported session is.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportedSession {
    /// The agent's discovery slug.
    pub agent: String,
    pub session_id: String,
    pub wsl_distro: Option<String>,
    pub title: Option<String>,
    /// Working directory the session ran in.
    pub cwd: Option<String>,
    /// `cli`, `ide_desktop`, or `unknown`.
    pub surface: String,
    /// ISO-8601 stamp of the transcript's most recent activity.
    pub last_activity: Option<String>,
    /// Where the provider's transcript lives — a reference, never a copy.
    pub source_path: Option<String>,
}

/// One exported session.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionExport {
    pub format: &'static str,
    pub format_version: u32,
    pub exported_at: String,
    pub app_version: String,
    /// Review date of the pricing catalog the cost estimate came from, so a
    /// figure can be interpreted later.
    pub pricing_catalog_version: &'static str,
    pub content_notice: &'static str,
    pub session: ExportedSession,
    /// The engine's per-session metrics, or absent when the transcript could
    /// not be analyzed.
    pub metrics: Option<SessionMetrics>,
    pub cost: Option<SessionCost>,
    pub models: Vec<String>,
    pub skills: Vec<SkillUse>,
    pub orchestration: Option<OrchestrationStatus>,
}

impl SessionExport {
    /// Serialize as the pretty JSON the file holds.
    pub fn to_json(&self) -> anyhow::Result<String> {
        Ok(serde_json::to_string_pretty(self)?)
    }

    /// Assemble a document from an analysis and the session's stored metadata.
    pub fn new(
        app_version: String,
        session: ExportedSession,
        analysis: &crate::analytics::SessionAnalysis,
    ) -> SessionExport {
        SessionExport {
            format: FORMAT,
            format_version: FORMAT_VERSION,
            exported_at: crate::store::now_rfc3339(),
            app_version,
            pricing_catalog_version: antiburn_local::pricing::PRICING_CATALOG_VERSION,
            content_notice: CONTENT_NOTICE,
            session,
            metrics: analysis.metrics.clone(),
            cost: analysis.cost,
            models: analysis.models.clone(),
            skills: analysis.skills.clone(),
            orchestration: analysis.orchestration.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analytics::SessionAnalysis;

    fn session() -> ExportedSession {
        ExportedSession {
            agent: "claude-code".into(),
            session_id: "abc".into(),
            wsl_distro: None,
            title: Some("Wire the popover".into()),
            cwd: Some("/home/avery/code/widgets".into()),
            surface: "cli".into(),
            last_activity: Some("2026-08-12T09:00:00Z".into()),
            source_path: Some("/home/avery/.claude/projects/widgets/abc.jsonl".into()),
        }
    }

    #[test]
    fn the_document_references_the_transcript_without_copying_it() {
        let export = SessionExport::new("0.1.0".into(), session(), &SessionAnalysis::unavailable());
        let json = export.to_json().unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert_eq!(value["format"], FORMAT);
        assert_eq!(value["formatVersion"], FORMAT_VERSION);
        assert_eq!(
            value["session"]["sourcePath"],
            "/home/avery/.claude/projects/widgets/abc.jsonl"
        );
        // The contract is stated in the file, not just in this module's docs.
        assert!(
            value["contentNotice"]
                .as_str()
                .is_some_and(|notice| notice.contains("derived analysis only"))
        );
    }

    #[test]
    fn an_unanalyzable_session_still_exports_its_identity() {
        let export = SessionExport::new("0.1.0".into(), session(), &SessionAnalysis::unavailable());
        let value: serde_json::Value = serde_json::from_str(&export.to_json().unwrap()).unwrap();
        assert_eq!(value["session"]["sessionId"], "abc");
        assert!(value["metrics"].is_null());
        assert!(value["cost"].is_null());
        assert_eq!(value["skills"].as_array().map(Vec::len), Some(0));
    }

    #[test]
    fn the_document_carries_the_catalog_version_its_costs_came_from() {
        let export = SessionExport::new("0.1.0".into(), session(), &SessionAnalysis::unavailable());
        assert_eq!(
            export.pricing_catalog_version,
            antiburn_local::pricing::PRICING_CATALOG_VERSION
        );
    }
}
