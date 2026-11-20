use std::fs;

use rusqlite::Connection;
use tempfile::{TempDir, tempdir};

use super::{SearchSession, get_local_session};
use crate::{checks::discovery::database_path, config::Config};

fn fixture() -> (TempDir, Config) {
    let root = tempdir().unwrap();
    let config = Config {
        data_root: Some(root.path().to_path_buf()),
        ..Config::defaults()
    };
    let path = database_path(&config, root.path().to_path_buf());
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let connection = Connection::open(path).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE session (
                 environment_key TEXT NOT NULL,
                 agent TEXT NOT NULL,
                 session_id TEXT NOT NULL,
                 title TEXT,
                 title_source TEXT,
                 updated_at_epoch INTEGER,
                 PRIMARY KEY (environment_key, agent, session_id)
             ) STRICT;
             INSERT INTO session VALUES
                 ('native', 'pi', 'shared-id', 'Local title', 'userRename', 10),
                 ('wsl:example', 'pi', 'shared-id', 'Hidden excerpt', 'firstMessage', 20),
                 ('native', 'codex', 'shared-id', NULL, NULL, 30),
                 ('native', 'pi', 'shared-id-longer', 'Title only', 'explicit', NULL);",
        )
        .unwrap();
    (root, config)
}

async fn get(
    config: &Config,
    session_id: &str,
    agent: Option<&str>,
    environment: Option<&str>,
) -> Result<SearchSession, String> {
    get_local_session(
        session_id.to_owned(),
        agent.map(str::to_owned),
        environment.map(str::to_owned),
        config.clone(),
    )
    .await
}

#[tokio::test]
async fn reads_only_metadata_for_the_exact_session_identity() {
    let (root, config) = fixture();
    let path = database_path(&config, root.path().to_path_buf());
    let before = fs::read(&path).unwrap();
    let session = get(&config, "shared-id", Some("pi"), Some("native"))
        .await
        .unwrap();
    assert_eq!(session.identity.session_id, "shared-id");
    assert_eq!(session.identity.agent, "pi");
    assert_eq!(session.identity.environment_key, "native");
    assert_eq!(session.title.as_deref(), Some("Local title"));
    assert_eq!(session.updated_at_epoch, Some(10));

    let session = get(&config, "shared-id", None, Some("wsl:example"))
        .await
        .unwrap();
    assert_eq!(session.identity.agent, "pi");
    assert_eq!(session.identity.environment_key, "wsl:example");
    assert_eq!(session.title, None);
    assert_eq!(session.updated_at_epoch, Some(20));

    let session = get(&config, "shared-id", Some("codex"), None)
        .await
        .unwrap();
    assert_eq!(session.identity.environment_key, "native");
    assert_eq!(session.identity.agent, "codex");

    let session = get(&config, "shared-id-longer", None, None).await.unwrap();
    assert_eq!(session.title.as_deref(), Some("Title only"));
    assert_eq!(session.updated_at_epoch, None);
    assert_eq!(fs::read(path).unwrap(), before);
}

#[tokio::test]
async fn rejects_missing_ids_and_mismatched_selectors_without_fallback() {
    let (_root, config) = fixture();
    for (session_id, agent, environment) in [
        ("shared", None, None),
        ("SHARED-ID", None, None),
        ("Title only", None, None),
        ("shared-id' OR 1 = 1 --", None, None),
        ("shared-id", Some("PI"), None),
        ("shared-id", None, Some("NATIVE")),
        ("shared-id", Some("codex"), Some("wsl:example")),
    ] {
        let error = get(&config, session_id, agent, environment)
            .await
            .unwrap_err();
        assert_eq!(
            error,
            format!("session ID {session_id:?} was not found in the local Antiburn database")
        );
    }
}

#[tokio::test]
async fn rejects_ambiguity_across_agents_or_environments() {
    let (_root, config) = fixture();
    for (agent, environment) in [(None, None), (Some("pi"), None), (None, Some("native"))] {
        let error = get(&config, "shared-id", agent, environment)
            .await
            .unwrap_err();
        assert_eq!(
            error,
            "session ID \"shared-id\" matches multiple local Antiburn sessions; use --agent and --environment to select one"
        );
    }
}

#[tokio::test]
async fn applies_the_configured_sqlite_length_limit() {
    let (root, mut config) = fixture();
    let title = "x".repeat(80 * 1024);
    let path = database_path(&config, root.path().to_path_buf());
    Connection::open(path)
        .unwrap()
        .execute(
            "UPDATE session SET title = ?1 WHERE session_id = 'shared-id-longer'",
            [&title],
        )
        .unwrap();

    let error = get(&config, "shared-id-longer", None, None)
        .await
        .unwrap_err();
    assert!(error.starts_with("could not read local Antiburn session metadata:"));

    config.local_search_sqlite_length_limit_bytes = 128 * 1024;
    let session = get(&config, "shared-id-longer", None, None).await.unwrap();
    assert_eq!(session.title.as_deref(), Some(title.as_str()));
}
