use super::*;

fn with_client(id: &str, updated_at: i64, client: &str) -> SessionRecord {
    let mut record = session(id, updated_at);
    record.client = client.into();
    record
}

#[test]
fn claude_client_counts_group_recent_local_claude_sessions() {
    const NOW: i64 = 2_000_000_000;
    let since = NOW - 7 * 86_400;
    let store = store();
    let mut codex = with_client("codex", NOW, "unknown");
    codex.key.agent = "codex".into();
    let mut remote = with_client("remote", NOW, "unknown");
    remote.key.environment_key = "ssh:host".into();
    let mut wsl = with_client("wsl", NOW, "vscode");
    wsl.key.environment_key = "wsl:ubuntu".into();
    store
        .upsert_sessions(
            &[
                with_client("cli-a", NOW, "cli"),
                with_client("cli-b", NOW - 86_400, "cli"),
                with_client("desktop", NOW, "claude_desktop"),
                with_client("old", since - 1, "claude_desktop"),
                with_client("legacy", NOW, "unknown"),
                codex,
                remote,
                wsl,
            ],
            &[],
        )
        .unwrap();

    assert_eq!(
        store.claude_client_counts_since(since).unwrap(),
        vec![
            ("claude_desktop".to_string(), 1),
            ("cli".to_string(), 2),
            ("unknown".to_string(), 1),
            ("vscode".to_string(), 1),
        ]
    );
}

#[test]
fn a_rescan_without_a_client_keeps_the_known_client() {
    let store = store();
    store
        .upsert_sessions(&[with_client("s", 10, "claude_desktop")], &[])
        .unwrap();
    store
        .upsert_sessions(&[with_client("s", 20, "unknown")], &[])
        .unwrap();
    let record = store
        .session(&SessionKey::new("native", "claude-code", "s"))
        .unwrap()
        .unwrap();
    assert_eq!(record.client, "claude_desktop");
}
