//! Versioned, read-only session collection over a user-owned SSH connection.

pub mod collector;
mod evidence;
pub mod export;
pub mod transport;

use antiburn_local::analysis::SessionMetrics;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

pub const PROTOCOL_VERSION: u32 = 1;
pub const MAX_REQUEST_BYTES: usize = 8 * 1024;
pub const MAX_RESPONSE_BYTES: u64 = 8 * 1024 * 1024;
pub const MAX_SESSIONS: usize = 200;
pub const LOOKBACK_SECS: i64 = 7 * 24 * 60 * 60;
pub const SUPPORTED_AGENTS: &[&str] = &["claude-code", "codex"];
pub const MAX_TITLE_CHARS: usize = 200;
pub const MAX_CWD_CHARS: usize = 1024;
pub const MAX_HELPER_VERSION_CHARS: usize = 64;
/// A valid stdio export request exits with this code when its evidence is rejected.
pub const SESSION_REJECTED_EXIT_CODE: i32 = 65;

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "operation", rename_all = "camelCase", deny_unknown_fields)]
pub enum Request {
    /// Reports compatibility without reading session content.
    Hello {
        version: u32,
    },
    List {
        version: u32,
    },
    Export {
        version: u32,
        agent: String,
        session_id: String,
        known: Option<String>,
    },
    Analyze {
        version: u32,
        agent: String,
        session_id: String,
    },
}

impl Request {
    pub fn validate(&self) -> anyhow::Result<()> {
        let version = match self {
            Self::Hello { .. } => return Ok(()),
            Self::List { version } => *version,
            Self::Export {
                version,
                agent,
                session_id,
                known,
            } => {
                validate_session_identity(agent, session_id)?;
                if let Some(known) = known {
                    anyhow::ensure!(
                        known.len() == 64 && known.bytes().all(|byte| byte.is_ascii_hexdigit()),
                        "Invalid bundle signature"
                    );
                }
                *version
            }
            Self::Analyze {
                version,
                agent,
                session_id,
            } => {
                validate_session_identity(agent, session_id)?;
                *version
            }
        };
        anyhow::ensure!(
            version == PROTOCOL_VERSION,
            "Remote helper protocol mismatch; update the helper"
        );
        Ok(())
    }
}

fn validate_session_identity(agent: &str, session_id: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        SUPPORTED_AGENTS.contains(&agent),
        "Unsupported remote agent"
    );
    anyhow::ensure!(
        export::safe_identity(session_id),
        "Invalid session identity"
    );
    Ok(())
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProtocolLimits {
    pub max_request_bytes: u64,
    pub max_response_bytes: u64,
    pub max_sessions: u64,
    pub lookback_secs: i64,
    pub max_bundle_files: u64,
    pub max_bundle_bytes: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Hello {
    pub version: u32,
    pub helper_version: String,
    pub platform: String,
    pub architecture: String,
    pub supported_agents: Vec<String>,
    pub limits: ProtocolLimits,
}

impl Hello {
    pub fn current() -> Self {
        Self {
            version: PROTOCOL_VERSION,
            helper_version: env!("CARGO_PKG_VERSION").to_owned(),
            platform: std::env::consts::OS.to_owned(),
            architecture: std::env::consts::ARCH.to_owned(),
            supported_agents: SUPPORTED_AGENTS
                .iter()
                .map(|agent| (*agent).to_owned())
                .collect(),
            limits: ProtocolLimits {
                max_request_bytes: MAX_REQUEST_BYTES as u64,
                max_response_bytes: MAX_RESPONSE_BYTES,
                max_sessions: MAX_SESSIONS as u64,
                lookback_secs: LOOKBACK_SECS,
                max_bundle_files: export::MAX_FILES as u64,
                max_bundle_bytes: export::MAX_BUNDLE_BYTES,
            },
        }
    }

    pub fn validate_compatibility(&self) -> anyhow::Result<()> {
        anyhow::ensure!(self.version == PROTOCOL_VERSION, "Protocol mismatch");
        anyhow::ensure!(self.platform == "linux", "Unsupported remote platform");
        anyhow::ensure!(
            matches!(self.architecture.as_str(), "x86_64" | "aarch64"),
            "Unsupported remote architecture"
        );
        anyhow::ensure!(
            SUPPORTED_AGENTS
                .iter()
                .all(|agent| self.supported_agents.iter().any(|actual| actual == agent)),
            "Helper does not support the required agents"
        );
        anyhow::ensure!(
            self.limits == Hello::current().limits,
            "Helper limits do not match the protocol"
        );
        // The desktop shows this version to the reader. The host account owner
        // controls the helper binary, so bound it like the other remote text.
        anyhow::ensure!(
            !self.helper_version.is_empty()
                && self
                    .helper_version
                    .chars()
                    .nth(MAX_HELPER_VERSION_CHARS)
                    .is_none()
                && self
                    .helper_version
                    .chars()
                    .all(|character| character.is_ascii_graphic()),
            "Helper reported an unusable version"
        );
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RemoteSession {
    pub agent: String,
    pub session_id: String,
    pub title: String,
    pub cwd: Option<String>,
    pub surface: String,
    pub updated_at: Option<i64>,
}

impl RemoteSession {
    pub fn validate(&self) -> anyhow::Result<()> {
        validate_session_identity(&self.agent, &self.session_id)?;
        anyhow::ensure!(
            self.title.chars().nth(MAX_TITLE_CHARS).is_none(),
            "Remote session title exceeds limit"
        );
        anyhow::ensure!(
            self.cwd
                .as_deref()
                .is_none_or(|cwd| cwd.chars().nth(MAX_CWD_CHARS).is_none()),
            "Remote session working directory exceeds limit"
        );
        anyhow::ensure!(
            matches!(self.surface.as_str(), "cli" | "ide_desktop" | "unknown"),
            "Unsupported remote session surface"
        );
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Snapshot {
    pub version: u32,
    pub collected_at: i64,
    pub sessions: Vec<RemoteSession>,
    pub truncated: bool,
    pub skipped: usize,
    pub lookback_secs: i64,
}

impl Snapshot {
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(self.version == PROTOCOL_VERSION, "Protocol mismatch");
        anyhow::ensure!(
            self.lookback_secs == LOOKBACK_SECS,
            "Remote session lookback does not match the protocol"
        );
        anyhow::ensure!(
            self.sessions.len() <= MAX_SESSIONS,
            "Remote session list exceeds limit"
        );
        let mut identities = HashSet::with_capacity(self.sessions.len());
        for session in &self.sessions {
            session.validate()?;
            anyhow::ensure!(
                identities.insert((session.agent.as_str(), session.session_id.as_str())),
                "Remote session list contains a duplicate identity"
            );
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Analysis {
    pub version: u32,
    pub collected_at: i64,
    pub session: RemoteSession,
    pub metrics: SessionMetrics,
    pub coverage: String,
}

pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hello_reports_the_versioned_bounds() {
        let hello = Hello::current();
        assert_eq!(hello.version, PROTOCOL_VERSION);
        assert_eq!(hello.limits.max_request_bytes, 8 * 1024);
        assert_eq!(hello.limits.max_response_bytes, 8 * 1024 * 1024);
        assert_eq!(hello.limits.max_sessions, 200);
        assert_eq!(hello.limits.lookback_secs, 7 * 24 * 60 * 60);
        assert_eq!(hello.limits.max_bundle_files, 512);
        assert_eq!(hello.limits.max_bundle_bytes, 1024 * 1024 * 1024);
    }

    #[test]
    fn hello_negotiates_before_strict_version_validation() {
        assert!(Request::Hello { version: 999 }.validate().is_ok());
        assert!(Request::List { version: 999 }.validate().is_err());
    }

    #[test]
    fn request_rejects_path_like_session_ids() {
        let request = Request::Export {
            version: PROTOCOL_VERSION,
            agent: "claude-code".to_owned(),
            session_id: "../../secret".to_owned(),
            known: None,
        };
        assert!(request.validate().is_err());
    }

    fn remote_session() -> RemoteSession {
        RemoteSession {
            agent: "codex".to_owned(),
            session_id: "session-1".to_owned(),
            title: "Session".to_owned(),
            cwd: Some("/workspace".to_owned()),
            surface: "cli".to_owned(),
            updated_at: Some(1),
        }
    }

    fn snapshot(session: RemoteSession) -> Snapshot {
        Snapshot {
            version: PROTOCOL_VERSION,
            collected_at: 1,
            sessions: vec![session],
            truncated: false,
            skipped: 0,
            lookback_secs: LOOKBACK_SECS,
        }
    }

    #[test]
    fn snapshot_validation_enforces_identity_uniqueness_and_protocol_bounds() {
        let session = remote_session();
        assert!(snapshot(session.clone()).validate().is_ok());

        let mut duplicate = snapshot(session.clone());
        duplicate.sessions.push(session);
        assert!(duplicate.validate().is_err());

        let mut wrong_lookback = snapshot(remote_session());
        wrong_lookback.lookback_secs -= 1;
        assert!(wrong_lookback.validate().is_err());

        let mut oversized = snapshot(remote_session());
        oversized.sessions = (0..=MAX_SESSIONS)
            .map(|index| RemoteSession {
                session_id: format!("session-{index}"),
                ..remote_session()
            })
            .collect();
        assert!(oversized.validate().is_err());
    }

    #[test]
    fn hello_validation_rejects_an_unusable_helper_version() {
        // The desktop renders this string, and the remote host supplies it.
        let mut compatible = Hello::current();
        compatible.platform = "linux".to_owned();
        compatible.architecture = "x86_64".to_owned();
        assert!(compatible.validate_compatibility().is_ok());

        for unusable in [
            String::new(),
            "x".repeat(MAX_HELPER_VERSION_CHARS + 1),
            "0.9.0\ninjected".to_owned(),
            "0.9.0\u{7f}".to_owned(),
        ] {
            let hello = Hello {
                helper_version: unusable,
                ..compatible.clone()
            };
            assert!(hello.validate_compatibility().is_err());
        }
    }

    #[test]
    fn remote_session_validation_rejects_unsupported_or_oversized_metadata() {
        let mut session = remote_session();
        session.agent = "other".to_owned();
        assert!(session.validate().is_err());

        let mut session = remote_session();
        session.session_id = "../escape".to_owned();
        assert!(session.validate().is_err());

        let mut session = remote_session();
        session.title = "x".repeat(MAX_TITLE_CHARS + 1);
        assert!(session.validate().is_err());

        let mut session = remote_session();
        session.cwd = Some("x".repeat(MAX_CWD_CHARS + 1));
        assert!(session.validate().is_err());

        let mut session = remote_session();
        session.surface = "remote-controlled".to_owned();
        assert!(session.validate().is_err());
    }
}
