#![cfg(all(feature = "cloud", debug_assertions, unix))]

use std::{
    io::{BufRead, BufReader, ErrorKind, Write},
    net::{TcpListener, TcpStream},
    os::unix::{fs::PermissionsExt, net::UnixListener},
    path::PathBuf,
    process::{Command, Output},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use rusqlite::{Connection, params};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tempfile::TempDir;

const WORKSPACE: &str = "18bb7532-8c59-45bc-b4b7-fc11ad27c5c1";
const SESSION: &str = "be5fdb34-fd2d-4aad-a1ac-9369a4ca5a15";
const OTHER: &str = "2f22d7cc-0b57-4080-a61a-0bd01e6a56bd";
const THIRD: &str = "ba31b6f6-6541-4767-91ab-b22b68c760dd";
const REVISION: &str = "e352e672-d152-4e97-a063-428bdeefb26a";
const KEY: &str = "synthetic-device-read-token";
const ROUTE_PREFIX: &str = "/v1/workspaces";
const DOCUMENT_BYTES: usize = 20 * 1024 * 1024;

struct Fixture {
    root: TempDir,
    config: PathBuf,
    origin: String,
}

impl Fixture {
    fn new(origin: &str) -> Self {
        let root = tempfile::tempdir_in(std::fs::canonicalize("/tmp").unwrap()).unwrap();
        std::fs::create_dir(root.path().join("ai.antiburn.desktop")).unwrap();
        let config = root.path().join("cli.json");
        std::fs::write(
            &config,
            json!({
                "data_root": root.path(),
                "cloud": {"workspace_id": WORKSPACE}
            })
            .to_string(),
        )
        .unwrap();
        Self {
            root,
            config,
            origin: origin.into(),
        }
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_antiburn"));
        command.arg("--config").arg(&self.config);
        command
    }

    fn run(&self, args: &[&str]) -> Output {
        self.run_connected(self.command().args(args).arg("--json"))
    }

    fn run_connected(&self, command: &mut Command) -> Output {
        self.run_with_broker_reply(
            command,
            json!({
                "origin": self.origin,
                "read_token": KEY,
                "personal_workspace_id": WORKSPACE
            }),
        )
    }

    fn run_with_broker_reply(&self, command: &mut Command, body: Value) -> Output {
        let path = self
            .root
            .path()
            .join("ai.antiburn.desktop/cloud-device.sock");
        let listener = UnixListener::bind(&path).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        listener.set_nonblocking(true).unwrap();
        let finished = AtomicBool::new(false);
        let output = thread::scope(|scope| {
            scope.spawn(|| {
                while !finished.load(Ordering::Acquire) {
                    match listener.accept() {
                        Ok((mut stream, _)) => {
                            stream.set_nonblocking(false).unwrap();
                            stream
                                .set_read_timeout(Some(Duration::from_secs(2)))
                                .unwrap();
                            let mut request = String::new();
                            BufReader::new(&stream).read_line(&mut request).unwrap();
                            assert_eq!(request, "device-read-session-v1\n");
                            writeln!(stream, "{body}").unwrap();
                            break;
                        }
                        Err(error) if error.kind() == ErrorKind::WouldBlock => {
                            thread::park_timeout(Duration::from_millis(5));
                        }
                        Err(error) => panic!("{error}"),
                    }
                }
            });
            let output = command.output();
            finished.store(true, Ordering::Release);
            output.unwrap()
        });
        std::fs::remove_file(path).unwrap();
        output
    }

    fn set_cloud_option(&self, field: &str, value: Value) {
        let mut config: Value =
            serde_json::from_slice(&std::fs::read(&self.config).unwrap()).unwrap();
        config["cloud"][field] = value;
        std::fs::write(&self.config, config.to_string()).unwrap();
    }

    fn local_database(&self, origin: &str) -> PathBuf {
        let directory = self.root.path().join("ai.antiburn.desktop");
        let database = directory.join("antiburn.sqlite3");
        let db = Connection::open(&database).unwrap();
        db.execute_batch(
            "CREATE TABLE session (
                 environment_key TEXT NOT NULL, agent TEXT NOT NULL, session_id TEXT NOT NULL,
                 title TEXT, title_source TEXT, updated_at_epoch INTEGER,
                 source_kind TEXT, source_label TEXT, source_format TEXT,
                 PRIMARY KEY(environment_key, agent, session_id)
             ) STRICT;
               CREATE TABLE cloud_sync_service_scope (
                 account_id TEXT NOT NULL,
                 destination_id TEXT NOT NULL,
                 resource_origin TEXT NOT NULL,
                 source_kind TEXT NOT NULL,
                 root_path TEXT NOT NULL,
                 source_scope_id TEXT NOT NULL UNIQUE,
                 PRIMARY KEY(account_id, destination_id, resource_origin, source_kind, root_path)
              ) STRICT;
              CREATE TABLE cloud_sync_source (
                 account_id TEXT NOT NULL,
                 destination_id TEXT NOT NULL,
                 source_id TEXT NOT NULL,
                 record_json TEXT NOT NULL CHECK(length(record_json) <= 16384),
                 PRIMARY KEY(account_id, destination_id, source_id)
             ) STRICT;",
        )
        .unwrap();
        let project = self.root.path().join("project");
        for (environment, id, title, time) in [
            ("native", "sample-one", "Proven local", Some(30)),
            ("wsl:synthetic", "sample-one", "Different source", Some(20)),
            ("native", "sample-two", "Unproven local", Some(15)),
            ("native", "sample-three", "Old receipt", None),
            ("native", SESSION, "Local only", None),
        ] {
            let source = project.join(format!("{id}.jsonl"));
            db.execute(
                "INSERT INTO session VALUES (?1, 'claude-code', ?2, ?3, 'explicit', ?4, 'file', ?5, '\"claude_jsonl\"')",
                params![environment, id, title, time, source.to_str().unwrap()],
            )
            .unwrap();
        }
        db.execute(
            r#"INSERT INTO cloud_sync_service_scope VALUES
                 ('synthetic-owner', ?1, ?2, '"claude_jsonl"', ?3, 'scope')"#,
            params![WORKSPACE, origin, project.to_str().unwrap()],
        )
        .unwrap();
        for (id, receipt_origin, observed) in [
            ("scope/sample-one", origin, true),
            ("scope/sample-two", "https://192.0.2.1", true),
            ("scope/sample-three", origin, false),
        ] {
            insert_receipt(&db, id, receipt_origin, observed);
        }
        database
    }
}

#[test]
fn rejects_a_legacy_broker_reply_with_full_device_proof() {
    let fixture = Fixture::new("http://127.0.0.1:1");
    let output = fixture.run_with_broker_reply(
        fixture
            .command()
            .args(["search", "test", "--source", "cloud"]),
        json!({
            "origin": fixture.origin,
            "access_token": "synthetic-full-device-bearer",
            "personal_workspace_id": WORKSPACE
        }),
    );
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert_eq!(
        String::from_utf8_lossy(&output.stderr).trim(),
        "antiburn: could not read the local device connection"
    );
}

fn insert_receipt(db: &Connection, id: &str, origin: &str, observed: bool) {
    let mut record = json!({
        "resource_origin": origin,
        "fingerprint": {
            "account_id": "synthetic-owner",
            "destination_id": WORKSPACE,
            "source_id": id,
            "source_fingerprint": "b".repeat(64),
        },
        "suppressed": false,
    });
    if observed {
        record["observation_key"] = json!("synthetic-observation");
    }
    db.execute(
        "INSERT INTO cloud_sync_source VALUES ('synthetic-owner', ?1, ?2, ?3)",
        params![WORKSPACE, id, record.to_string()],
    )
    .unwrap();
}

fn serve(response: String) -> (String, JoinHandle<Vec<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    (origin, serve_on(listener, response))
}

fn serve_on(listener: TcpListener, response: String) -> JoinHandle<Vec<String>> {
    serve_with(listener, move |stream| {
        stream.write_all(response.as_bytes()).unwrap();
    })
}

fn serve_with(
    listener: TcpListener,
    respond: impl FnOnce(&mut TcpStream) + Send + 'static,
) -> JoinHandle<Vec<String>> {
    listener.set_nonblocking(true).unwrap();
    thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(error) if error.kind() == ErrorKind::WouldBlock => {
                    assert!(Instant::now() < deadline, "Cloud request did not arrive");
                    thread::park_timeout(Duration::from_millis(10));
                }
                Err(error) => panic!("{error}"),
            }
        };
        stream.set_nonblocking(false).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        stream
            .set_write_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let lines: Vec<_> = BufReader::new(&mut stream)
            .lines()
            .map(Result::unwrap)
            .take_while(|line| !line.is_empty())
            .collect();
        respond(&mut stream);
        lines
    })
}

fn http(status: &str, body: &str) -> String {
    format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

fn assert_cloud_request(server: JoinHandle<Vec<String>>, target: &str) {
    let lines = server.join().unwrap();
    assert_eq!(lines[0], format!("GET {target} HTTP/1.1"));
    assert!(lines.contains(&format!("authorization: Bearer {KEY}")));
    assert!(!lines.iter().any(|line| line.starts_with("cookie:")));
    assert!(!lines.iter().any(|line| line.starts_with("origin:")));
}

fn read_success(output: Output) -> Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    serde_json::from_slice(&output.stdout).unwrap()
}

fn assert_private_error(output: Output, expected: &str) {
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let error = String::from_utf8(output.stderr).unwrap();
    assert!(error.contains(expected), "{error}");
    assert!(!error.contains(KEY), "{error}");
    assert!(!error.contains("private-synthetic-body"), "{error}");
}

fn item(id: &str, title: &str, key: Option<&str>) -> Value {
    let source_key = key.map(|value| {
        Sha256::digest(value.as_bytes())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    });
    json!({
        "id": id,
        "title": title,
        "agent": {"name": "claude-code", "version": null},
        "started_at": "1970-01-01T00:00:15Z",
        "source_key": source_key
    })
}

fn list(workspace: &str, items: Vec<Value>) -> String {
    http(
        "200 OK",
        &json!({"workspace_id": workspace, "items": items, "page": {"next_cursor": null}})
            .to_string(),
    )
}

#[test]
fn rejects_unsafe_origin_before_requesting_with_the_key() {
    for origin in [
        "http://192.0.2.1",
        "https://private-synthetic-body@192.0.2.1",
        "https://192.0.2.1/private-synthetic-body",
        "https://192.0.2.1?private-synthetic-body",
        "https://192.0.2.1#private-synthetic-body",
    ] {
        let fixture = Fixture::new(origin);
        assert_private_error(
            fixture.run(&["search", "sample", "--source", "cloud"]),
            "canonical HTTPS origin",
        );
    }
}

#[test]
fn a_cloud_access_denial_never_returns_local_content() {
    let (origin, server) = serve(http("403 Forbidden", "private-synthetic-body"));
    let fixture = Fixture::new(&origin);
    fixture.local_database(&origin);
    let project = fixture.root.path().join("project");
    std::fs::create_dir(&project).unwrap();
    std::fs::write(
        project.join(format!("{SESSION}.jsonl")),
        format!(
            "{{\"type\":\"user\",\"sessionId\":\"{SESSION}\",\"uuid\":\"first\",\"parentUuid\":null,\
            \"message\":{{\"role\":\"user\",\"content\":\"Synthetic local content.\"}}}}\n"
        ),
    )
    .unwrap();
    let local = read_success(
        fixture
            .command()
            .args(["get", SESSION, "--source", "local", "--json"])
            .output()
            .unwrap(),
    );
    assert_eq!(local["source"], "local");
    assert_eq!(
        local["document"]["messages"][0]["parts"][0]["text"],
        "Synthetic local content."
    );
    let output = fixture.run(&["get", SESSION, "--source", "cloud"]);
    assert_private_error(output, "Cloud access denied");
    assert_cloud_request(
        server,
        &format!("{ROUTE_PREFIX}/{WORKSPACE}/sessions/{SESSION}/document"),
    );
}

#[test]
fn a_redirect_is_not_followed_or_included_in_the_error() {
    let redirect = "HTTP/1.1 307 Temporary Redirect\r\n\
                    Location: /private-synthetic-body\r\n\
                    Content-Length: 0\r\nConnection: close\r\n\r\n";
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let server = serve_on(listener.try_clone().unwrap(), redirect.into());
    let fixture = Fixture::new(&origin);
    fixture.set_cloud_option("request_timeout_secs", json!(1));
    let output = fixture.run(&["search", "sample", "--source", "cloud", "--limit", "2"]);
    assert_private_error(output, "Cloud request failed");
    assert_cloud_request(
        server,
        &format!("{ROUTE_PREFIX}/{WORKSPACE}/sessions?q=sample&page%5Bsize%5D=2"),
    );
    assert!(
        matches!(listener.accept(), Err(error) if error.kind() == ErrorKind::WouldBlock),
        "the CLI connected to the redirect target"
    );
}

#[test]
fn rejects_out_of_range_cloud_timeouts_before_network_io() {
    for field in ["connect_timeout_secs", "request_timeout_secs"] {
        let fixture = Fixture::new("http://127.0.0.1:1");
        for value in [0, 301] {
            fixture.set_cloud_option(field, json!(value));
            assert_private_error(
                fixture.run(&["search", "sample", "--source", "cloud"]),
                &format!("cloud.{field} must be between 1 and 300"),
            );
        }
    }
}

#[test]
fn reports_cloud_timeout_without_returning_response_data() {
    for prefix in [
        "",
        "HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\nprivate-synthetic-body",
    ] {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let fixture = Fixture::new(&format!("http://{}", listener.local_addr().unwrap()));
        fixture.set_cloud_option("request_timeout_secs", json!(1));
        let (finished, wait) = mpsc::channel::<()>();
        let server = serve_with(listener, move |stream| {
            stream.write_all(prefix.as_bytes()).unwrap();
            assert_eq!(
                wait.recv_timeout(Duration::from_secs(5)),
                Err(mpsc::RecvTimeoutError::Disconnected)
            );
        });
        let output = fixture.run(&["get", SESSION, "--source", "cloud"]);
        drop(finished);
        assert_private_error(
            output,
            "Cloud request timed out; check cloud.request_timeout_secs",
        );
        assert_cloud_request(
            server,
            &format!("{ROUTE_PREFIX}/{WORKSPACE}/sessions/{SESSION}/document"),
        );
    }
}

#[test]
fn searches_cloud_without_opening_the_local_database() {
    let (origin, server) = serve(http(
        "200 OK",
        &json!({"items": [item(SESSION, "Cloud match", None)], "page": {"next_cursor": "another-page"}}).to_string(),
    ));
    let fixture = Fixture::new(&origin);
    let result = read_success(fixture.run(&["search", "A + B", "--source", "cloud"]));
    assert_eq!(result["source"], "cloud");
    assert_eq!(result["workspace_id"], WORKSPACE);
    assert_eq!(result["sessions"][0]["cloud"]["id"], SESSION);
    assert!(result["sources"].get("local").is_none());
    assert_eq!(result["sources"]["cloud"]["truncated"], true);
    assert_cloud_request(
        server,
        &format!("{ROUTE_PREFIX}/{WORKSPACE}/sessions?q=A+%2B+B&page%5Bsize%5D=20"),
    );
}

#[test]
fn default_search_stays_local_without_a_device_connection() {
    let fixture = Fixture::new("http://127.0.0.1:1");
    fixture.local_database("http://127.0.0.1:1");
    let result = read_success(
        fixture
            .command()
            .args(["search", "sample", "--json"])
            .output()
            .unwrap(),
    );
    assert_eq!(result["source"], "local");
    assert!(result["sources"].get("cloud").is_none());
    assert!(result["sessions"][0].get("local").is_some());
}

#[test]
fn rejects_an_unprotected_device_socket_but_local_search_ignores_it() {
    let fixture = Fixture::new("http://127.0.0.1:1");
    fixture.local_database("http://127.0.0.1:1");
    let path = fixture
        .root
        .path()
        .join("ai.antiburn.desktop/cloud-device.sock");
    let _listener = UnixListener::bind(&path).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o666)).unwrap();
    let run = |args: &[&str]| fixture.command().args(args).output().unwrap();
    assert_private_error(
        run(&["search", "sample", "--source", "cloud", "--json"]),
        "could not read the local device connection",
    );
    let local = read_success(run(&["search", "sample", "--source", "local", "--json"]));
    assert_eq!(local["source"], "local");
}

#[test]
fn merges_only_exact_accepted_source_bindings() {
    let mut older = item(OTHER, "Cloud two", Some("scope/sample-two"));
    older["started_at"] = json!("1970-01-01T00:00:15.100Z");
    let mut newer = item(THIRD, "Cloud three", Some("scope/sample-three"));
    newer["started_at"] = json!("1970-01-01T00:00:15.900Z");
    let (origin, server) = serve(list(
        WORKSPACE,
        vec![
            item(SESSION, "Cloud one", Some("scope/sample-one")),
            older,
            newer,
        ],
    ));
    let fixture = Fixture::new(&origin);
    let database = fixture.local_database(&origin);
    let before = std::fs::read(&database).unwrap();
    let result = read_success(fixture.run(&["search", "sample", "--limit", "6"]));
    assert_eq!(result["source"], "all");
    assert_eq!(result["sources"]["cloud"]["truncated"], false);
    assert_eq!(result["sources"]["local"]["truncated"], false);
    let matches = result["sessions"].as_array().unwrap();
    assert_eq!(matches.len(), 6);
    assert_eq!(matches[0]["local"]["identity"]["session_id"], "sample-one");
    assert_eq!(matches[0]["cloud"]["id"], SESSION);
    assert!(matches[0]["cloud"].get("source_key").is_none());
    assert_eq!(
        matches[1]["local"]["identity"]["environment_key"],
        "wsl:synthetic"
    );
    assert!(matches[1].get("cloud").is_none());
    assert_eq!(matches[2]["local"]["identity"]["session_id"], "sample-two");
    assert!(matches[2].get("cloud").is_none());
    assert_eq!(
        matches[3]["local"]["identity"]["session_id"],
        "sample-three"
    );
    assert!(matches[3].get("cloud").is_none());
    assert_eq!(matches[4]["cloud"]["id"], THIRD);
    assert!(matches[4].get("local").is_none());
    assert_eq!(matches[5]["cloud"]["id"], OTHER);
    assert!(matches[5].get("local").is_none());
    assert_cloud_request(
        server,
        &format!("{ROUTE_PREFIX}/{WORKSPACE}/sessions?q=sample&page%5Bsize%5D=6"),
    );
    assert_eq!(std::fs::read(database).unwrap(), before);
}

#[test]
fn opencode_merging_requires_the_exact_bound_database_path() {
    const SOURCE: &str = "scope-opencode/opencode/b3BlbmNvZGUuZGI/c2FtcGxl";
    let mut remote = item(SESSION, "Cloud OpenCode", Some(SOURCE));
    remote["agent"]["name"] = json!("opencode");
    let (origin, server) = serve(list(WORKSPACE, vec![remote.clone()]));
    let mut fixture = Fixture::new(&origin);
    let source_path = fixture.root.path().join("opencode.db");
    let other_path = fixture.root.path().join("another").join("opencode.db");
    let db = Connection::open(fixture.local_database(&origin)).unwrap();
    db.execute_batch("ALTER TABLE session ADD COLUMN fingerprint_source_path TEXT;")
        .unwrap();
    db.execute(
        r#"INSERT INTO session VALUES ('native', 'opencode', 'sample', 'Select this', 'explicit', 30,
                'providerDb', 'opencode:sample:open_code_v2_sqlite',
                 '"open_code_v2_sqlite"', ?1);"#,
        [source_path.to_str().unwrap()],
    )
    .unwrap();
    db.execute(
        r#"INSERT INTO cloud_sync_service_scope VALUES
              ('synthetic-owner', ?1, ?2, '"opencode_sqlite_v2"', ?3, 'scope-opencode')"#,
        params![WORKSPACE, origin, source_path.to_str().unwrap()],
    )
    .unwrap();
    insert_receipt(&db, SOURCE, &origin, true);
    let args = ["search", "Select this", "--source", "all"];
    let result = read_success(fixture.run(&args));
    assert_eq!(result["sessions"].as_array().unwrap().len(), 1);
    assert_eq!(
        result["sessions"][0]["local"]["identity"]["session_id"],
        "sample"
    );
    assert_eq!(result["sessions"][0]["cloud"]["id"], SESSION);
    let route = format!("{ROUTE_PREFIX}/{WORKSPACE}/sessions?q=Select+this&page%5Bsize%5D=20");
    assert_cloud_request(server, &route);

    let (new_origin, server) = serve(list(WORKSPACE, vec![remote]));
    fixture.origin.clone_from(&new_origin);
    db.execute(
        "UPDATE cloud_sync_service_scope SET resource_origin=?1 WHERE source_scope_id='scope-opencode'",
        [&new_origin],
    ).unwrap();
    db.execute("DELETE FROM cloud_sync_source WHERE source_id=?1", [SOURCE])
        .unwrap();
    insert_receipt(&db, SOURCE, &new_origin, true);
    db.execute(
        "UPDATE session SET fingerprint_source_path=?1 WHERE agent='opencode'",
        [other_path.to_str().unwrap()],
    )
    .unwrap();
    let result = read_success(fixture.run(&args));
    assert_eq!(result["sessions"].as_array().unwrap().len(), 2);
    assert_cloud_request(server, &route);
}

#[test]
fn explicit_combined_search_cannot_hide_cloud_access_denial() {
    let (origin, server) = serve(http("401 Unauthorized", "private-synthetic-body"));
    let fixture = Fixture::new(&origin);
    fixture.local_database(&origin);
    assert_private_error(
        fixture.run(&["search", "sample", "--source", "all"]),
        "Cloud access denied",
    );
    assert_cloud_request(
        server,
        &format!("{ROUTE_PREFIX}/{WORKSPACE}/sessions?q=sample&page%5Bsize%5D=20"),
    );
}

#[test]
fn rejects_the_wrong_workspace_and_duplicate_cloud_ids() {
    for response in [
        list(OTHER, vec![]),
        list(
            WORKSPACE,
            vec![item(SESSION, "a", None), item(SESSION, "b", None)],
        ),
    ] {
        let (origin, server) = serve(response);
        let fixture = Fixture::new(&origin);
        assert_private_error(
            fixture.run(&["search", "sample", "--source", "cloud"]),
            "does not match the request",
        );
        assert_cloud_request(
            server,
            &format!("{ROUTE_PREFIX}/{WORKSPACE}/sessions?q=sample&page%5Bsize%5D=20"),
        );
    }
}

fn document(session: &str, text: &str) -> Value {
    json!({
        "schema_version": 1,
        "source": "cloud",
        "workspace_id": WORKSPACE,
        "session_id": session,
        "revision": REVISION,
        "stale": true,
        "document": {
            "version": 1,
            "header": {"agent": {"name": "claude-code", "version": null}},
            "coverage": {"source": "unknown", "projection": "partial", "ordering": "presentation"},
            "messages": [
                {"id": "message:1", "role": "assistant",
                 "parts": [{"id": "part:1", "type":"text", "content_state":"complete", "text":text}]}
            ]
        }
    })
}

fn large_document_body(size: usize) -> (String, String) {
    let mut content = String::from("private-synthetic-body \"quoted\"\n");
    content.push_str(&"x".repeat(size - document(SESSION, &content).to_string().len()));
    let body = document(SESSION, &content).to_string();
    assert_eq!(body.len(), size);
    (body, content)
}

#[test]
fn reads_one_document_and_drops_unknown_fields() {
    let mut body = document(SESSION, "Synthetic reply.");
    body["environment_key"] = json!("synthetic-cloud-environment");
    body["future_envelope_field"] = json!("synthetic");
    body["document"]["messages"][0]["future_message_field"] = json!({"sample": 1});
    body["document"]["messages"][0]["parts"][0]["future_part_field"] = json!("synthetic");
    let (origin, server) = serve(http("200 OK", &body.to_string()));
    let fixture = Fixture::new(&origin);
    let result = read_success(fixture.run(&["get", SESSION, "--source", "cloud"]));
    assert_eq!(result, document(SESSION, "Synthetic reply."));
    assert_cloud_request(
        server,
        &format!("{ROUTE_PREFIX}/{WORKSPACE}/sessions/{SESSION}/document"),
    );
}

#[test]
fn preserves_projected_sections_and_semantic_numbers() {
    let mut body = document(SESSION, "Synthetic reply.");
    let number = "90071992547409931234567890";
    body["document"]["messages"][0]["parts"]
        .as_array_mut()
        .unwrap()
        .push(json!({
            "id": "part:2",
            "type": "tool_call",
            "name": "synthetic-tool",
            "content_state": "complete",
            "input": {"format": "json", "value": {"count": serde_json::from_str::<Value>(number).unwrap()}},
        }));
    body["document"]["messages"][0]["parts"]
        .as_array_mut()
        .unwrap()
        .push(json!({
            "id": "part:3",
            "type": "tool_result",
            "content_state": "complete",
            "tool_call_part_id": "part:2",
            "output": [{"format": "text", "value": "Synthetic result."}],
        }));
    body["document"]["native_only"] =
        json!([{"id": "native:1", "native_type": "synthetic", "reason": "unsupported"}]);
    body["document"]["usage"] = json!({"summaries": [{"id": "usage:1", "scope": {"kind": "session"}, "tokens": {"total": "42"}}]});
    body["document"]["coverage"]["limitations"] =
        json!([{"area": "messages", "reason": "synthetic_gap"}]);
    let (origin, server) = serve(http("200 OK", &body.to_string()));
    let fixture = Fixture::new(&origin);
    let result = read_success(fixture.run(&["get", SESSION, "--source", "cloud"]));
    assert_eq!(result, body);
    assert_cloud_request(
        server,
        &format!("{ROUTE_PREFIX}/{WORKSPACE}/sessions/{SESSION}/document"),
    );
}

#[test]
fn reads_a_document_at_the_wire_size_limit() {
    let mut content = "\u{7f}".repeat(14 * 1024 * 1024);
    content.push('\u{2028}');
    let padding = DOCUMENT_BYTES - document(SESSION, "").to_string().len() - content.len();
    content.push_str(&"x".repeat(padding));
    let body = document(SESSION, &content).to_string();
    assert_eq!(body.len(), DOCUMENT_BYTES);
    let args = ["get", SESSION, "--source", "cloud"];
    for (json, escaped_del, escaped_separator) in [
        (true, "\\u007f", "\\u2028"),
        (false, "\\u{7f}", "\\u{2028}"),
    ] {
        let (origin, server) = serve(http("200 OK", &body));
        let fixture = Fixture::new(&origin);
        let mut command = fixture.command();
        command.args(args);
        if json {
            command.arg("--json");
        }
        let output = fixture.run_connected(&mut command);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.stdout.len() > 4 * DOCUMENT_BYTES);
        let stdout = std::str::from_utf8(&output.stdout).unwrap();
        assert!(stdout.contains(escaped_del));
        assert!(stdout.contains(escaped_separator));
        assert!(!stdout.contains(['\u{7f}', '\u{2028}']));
        if json {
            let decoded = read_success(output);
            assert_eq!(
                decoded["document"]["messages"][0]["parts"][0]["text"],
                content
            );
        }
        assert_cloud_request(
            server,
            &format!("{ROUTE_PREFIX}/{WORKSPACE}/sessions/{SESSION}/document"),
        );
    }
}

#[test]
fn displays_document_status_and_escapes_text() {
    let mut body = document(SESSION, "Synthetic reply.\x1b[2J");
    body["environment_key"] = json!("synthetic-cloud-environment");
    body["document"]["header"]["title"] = json!("Synthetic title\nsecond line");
    body["document"]["coverage"]["source"] = json!("partial");
    let (origin, server) = serve(http("200 OK", &body.to_string()));
    let fixture = Fixture::new(&origin);
    let output = fixture.run_connected(
        fixture
            .command()
            .args(["get", SESSION, "--source", "cloud"]),
    );
    assert!(output.status.success(), "{:?}", output.stderr);
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.starts_with("Synthetic title\\nsecond line\n"));
    assert!(text.contains(&format!("Session {SESSION} (cloud)")));
    assert!(text.contains("Agent: claude-code"));
    assert!(!text.contains("Environment:"));
    assert!(text.contains(&format!("Revision: {REVISION}")));
    assert!(text.contains("Cloud returned an older publication."));
    assert!(text.contains("Source coverage: partial"));
    assert!(text.contains("Document projection is partial."));
    assert!(text.contains("Assistant: \"Synthetic reply.\\u{1b}[2J\""));
    assert!(!text.contains('\x1b'));
    assert_cloud_request(
        server,
        &format!("{ROUTE_PREFIX}/{WORKSPACE}/sessions/{SESSION}/document"),
    );
}

#[test]
fn rejects_wrong_document_identity_and_version() {
    for (field, value, expected) in [
        ("session_id", json!(OTHER), "does not match the request"),
        ("workspace_id", json!(OTHER), "does not match the request"),
        ("schema_version", json!(2), "unsupported document version"),
        ("source", json!("local"), "does not match the request"),
    ] {
        let mut body = document(SESSION, "Synthetic message.");
        body[field] = value;
        let (origin, server) = serve(http("200 OK", &body.to_string()));
        let fixture = Fixture::new(&origin);
        assert_private_error(
            fixture.run(&["get", SESSION, "--source", "cloud"]),
            expected,
        );
        assert_cloud_request(
            server,
            &format!("{ROUTE_PREFIX}/{WORKSPACE}/sessions/{SESSION}/document"),
        );
    }
}

#[test]
fn reports_cloud_version_errors_even_if_v1_fields_are_invalid() {
    let mut wrong_coverage_shape = document(SESSION, "private-synthetic-body");
    wrong_coverage_shape["document"]["coverage"] = json!(["partial"]);
    let mut unknown_envelope = wrong_coverage_shape.clone();
    unknown_envelope["schema_version"] = json!(256);
    unknown_envelope["document"] = json!(["changed document shape"]);
    let mut unknown_document = wrong_coverage_shape.clone();
    unknown_document["document"]["version"] = json!(u64::MAX);
    let mut compatible_shape = document(SESSION, "private-synthetic-body");
    compatible_shape["document"]["version"] = json!(2);
    let mut invalid_envelope = unknown_document.clone();
    invalid_envelope["schema_version"] = json!("invalid");
    for (body, expected) in [
        (unknown_envelope, "unsupported document version"),
        (unknown_document, "unsupported document version"),
        (compatible_shape, "unsupported document version"),
        (invalid_envelope, "returned an invalid response"),
        (wrong_coverage_shape, "returned an invalid response"),
    ] {
        let (origin, server) = serve(http("200 OK", &body.to_string()));
        let fixture = Fixture::new(&origin);
        assert_private_error(
            fixture.run(&["get", SESSION, "--source", "cloud"]),
            expected,
        );
        assert_cloud_request(
            server,
            &format!("{ROUTE_PREFIX}/{WORKSPACE}/sessions/{SESSION}/document"),
        );
    }
}

#[test]
fn rejects_overlong_cloud_requests_before_network_io() {
    let fixture = Fixture::new("http://127.0.0.1:1");
    assert_private_error(
        fixture.run(&["search", "sample", "--source", "cloud", "--limit", "51"]),
        "Cloud search allows 1 to 50 results and queries up to 200 bytes",
    );
    for args in [
        ["search", &"a".repeat(201), "--source", "cloud"],
        ["search", &"©".repeat(101), "--source", "cloud"],
    ] {
        assert_private_error(
            fixture.run(&args),
            "Cloud search allows queries up to 200 bytes; use --source local",
        );
    }
}

#[test]
fn overlong_default_search_requires_explicit_local_source() {
    let fixture = Fixture::new("http://127.0.0.1:1");
    let query = "a".repeat(201);
    let database = fixture.local_database(&fixture.origin);
    Connection::open(database)
        .unwrap()
        .execute(
            "UPDATE session SET title=?1 WHERE session_id=?2",
            params![query, SESSION],
        )
        .unwrap();

    assert_private_error(
        fixture.run(&["search", &query]),
        "Cloud search allows queries up to 200 bytes; use --source local",
    );
    let local = read_success(fixture.run(&["search", &query, "--source", "local"]));
    assert_eq!(local["source"], "local");
    assert_eq!(local["sessions"].as_array().unwrap().len(), 1);
    assert_eq!(
        local["sessions"][0]["local"]["identity"]["session_id"],
        SESSION
    );
}

#[test]
fn accepts_a_cloud_query_of_200_utf8_bytes() {
    let query = "©".repeat(100);
    let (origin, server) = serve(list(WORKSPACE, Vec::new()));
    let fixture = Fixture::new(&origin);
    let result = read_success(fixture.run(&["search", &query, "--source", "cloud"]));
    assert_eq!(result["sessions"], json!([]));
    assert_cloud_request(
        server,
        &format!(
            "{ROUTE_PREFIX}/{WORKSPACE}/sessions?q={}&page%5Bsize%5D=20",
            "%C2%A9".repeat(100)
        ),
    );
}

#[test]
fn rejects_an_oversized_declared_document_body_before_reading_it() {
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        DOCUMENT_BYTES + 1
    );
    let (origin, server) = serve(response);
    let fixture = Fixture::new(&origin);
    assert_private_error(
        fixture.run(&["get", SESSION, "--source", "cloud"]),
        "response exceeds the size limit",
    );
    assert_cloud_request(
        server,
        &format!("{ROUTE_PREFIX}/{WORKSPACE}/sessions/{SESSION}/document"),
    );
}

#[test]
fn rejects_an_oversized_chunked_document_body_without_showing_it() {
    let (body, _) = large_document_body(DOCUMENT_BYTES + 1);
    let response = format!(
        "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n{:x}\r\n{body}\r\n0\r\n\r\n",
        body.len()
    );
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let fixture = Fixture::new(&format!("http://{}", listener.local_addr().unwrap()));
    let server = serve_with(listener, move |stream| {
        if let Err(error) = stream.write_all(response.as_bytes()) {
            assert!(matches!(
                error.kind(),
                ErrorKind::BrokenPipe | ErrorKind::ConnectionReset | ErrorKind::ConnectionAborted
            ));
        }
    });
    assert_private_error(
        fixture.run(&["get", SESSION, "--source", "cloud"]),
        "response exceeds the size limit",
    );
    assert_cloud_request(
        server,
        &format!("{ROUTE_PREFIX}/{WORKSPACE}/sessions/{SESSION}/document"),
    );
}

#[test]
fn rejects_a_malformed_success_body_without_showing_it() {
    let (origin, server) = serve(http("200 OK", "{\"messages\":\"private-synthetic-body\"}"));
    let fixture = Fixture::new(&origin);
    assert_private_error(
        fixture.run(&["get", SESSION, "--source", "cloud"]),
        "returned an invalid response",
    );
    assert_cloud_request(
        server,
        &format!("{ROUTE_PREFIX}/{WORKSPACE}/sessions/{SESSION}/document"),
    );
}
