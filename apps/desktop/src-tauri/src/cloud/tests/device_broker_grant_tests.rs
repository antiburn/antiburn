use super::*;
use crate::cloud::{
    auth::{AccountSession, CAPTURE_SCOPE, Config, EligibleWorkspace, TokenRequest, Tokens},
    engine::Access,
    state::{Cleanup, Phase, StateStore},
    tests::{MemoryVault, session},
};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

struct Workspaces {
    personal_id: &'static str,
    current: Arc<AtomicBool>,
    cancel: bool,
}

impl crate::cloud::auth::Transport for Workspaces {
    fn exchange(&self, _: &Config, _: &TokenRequest<'_>) -> Result<Tokens, Failure> {
        panic!("a cached, unexpired token does not need to rotate")
    }

    fn session(&self, _: &Config, _: &str) -> Result<AccountSession, Failure> {
        panic!("a cached, unexpired token does not need to rotate")
    }

    fn workspaces(&self, _: &Config, bearer: &str) -> Result<Vec<EligibleWorkspace>, Failure> {
        assert_eq!(bearer, "synthetic-access");
        assert_ne!(
            self.personal_id, "",
            "capture-only grants cannot call this route"
        );
        if self.cancel {
            self.current.store(false, Ordering::Release);
        }
        let mut personal = session().workspace;
        personal.id = self.personal_id.into();
        Ok(vec![
            EligibleWorkspace {
                workspace: personal,
                membership_generation: None,
            },
            EligibleWorkspace {
                workspace: crate::cloud::auth::Workspace {
                    id: "synthetic-team".into(),
                    name: "Team".into(),
                    kind: WorkspaceKind::Team,
                },
                membership_generation: Some("synthetic-membership".into()),
            },
        ])
    }

    fn revoke(&self, _: &Config, _: &str) -> Result<(), Failure> {
        panic!("a workspace read does not revoke a grant")
    }
}

fn engine(directory: &std::path::Path, scope: &str, workspaces: Workspaces) -> Engine {
    let mut session = session();
    session.scope = scope.into();
    let config = Config::parse(&session.iss, &session.resource).unwrap();
    let state = StateStore::new(directory);
    let record = state
        .lock()
        .unwrap()
        .replace(Phase::Active, Cleanup::None)
        .unwrap();
    let mut engine = Engine::new(
        Some(config),
        Box::new(Arc::new(MemoryVault::default())),
        Box::new(workspaces),
        state,
    )
    .with_clock(|| 1);
    engine.access = Some(Access {
        token: "synthetic-access".into(),
        expires_at: session.access_expires_at,
        epoch: record.epoch,
        session,
    });
    engine
}

#[test]
fn returns_only_read_capable_grants_with_verified_workspace_ids() {
    let directory = tempfile::tempdir().unwrap();
    let current = Arc::new(AtomicBool::new(true));
    let mut engine = engine(
        directory.path(),
        CAPTURE_AND_SESSION_SCOPE,
        Workspaces {
            personal_id: "synthetic-personal",
            current: current.clone(),
            cancel: false,
        },
    );
    let (origin, bearer, personal, workspaces) = engine
        .cli_device_session(&|| current.load(Ordering::Acquire))
        .unwrap();
    assert_eq!(origin, "https://api.example");
    assert_eq!(bearer, "synthetic-access");
    assert_eq!(personal, "synthetic-personal");
    assert_eq!(workspaces, ["synthetic-personal", "synthetic-team"]);
}

#[test]
fn rejects_capture_only_grants_wrong_anchors_and_cancelled_reads() {
    for (scope, personal_id, cancel, expected) in [
        (CAPTURE_SCOPE, "", false, Failure::Unavailable),
        (
            CAPTURE_AND_SESSION_SCOPE,
            "wrong-personal",
            false,
            Failure::InvalidResponse,
        ),
        (
            CAPTURE_AND_SESSION_SCOPE,
            "synthetic-personal",
            true,
            Failure::Cancelled,
        ),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let current = Arc::new(AtomicBool::new(true));
        let mut engine = engine(
            directory.path(),
            scope,
            Workspaces {
                personal_id,
                current: current.clone(),
                cancel,
            },
        );
        assert!(matches!(
            engine.cli_device_session(&|| current.load(Ordering::Acquire)),
            Err(error) if error == expected
        ));
    }
}
