#![cfg(all(feature = "cloud", debug_assertions, unix))]

use std::{
    io::{BufRead, BufReader, ErrorKind, Write},
    net::{TcpListener, TcpStream},
    os::unix::{fs::PermissionsExt, net::UnixListener},
    path::PathBuf,
    process::{Command, Output},
    sync::atomic::{AtomicBool, Ordering},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use rusqlite::{Connection, params};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tempfile::TempDir;

const WORKSPACE: &str = "18bb7532-8c59-45bc-b4b7-fc11ad27c5c1";
const SESSION: &str = "be5fdb34-fd2d-4aad-a1ac-9369a4ca5a15";
const OTHER: &str = "2f22d7cc-0b57-4080-a61a-0bd01e6a56bd";
const THIRD: &str = "ba31b6f6-6541-4767-91ab-b22b68c760dd";
const REVISION: &str = "e352e672-d152-4e97-a063-428bdeefb26a";
const KEY: &str = "synthetic-device-access-token";
const CURSOR: &str = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
const ROUTE_PREFIX: &str = "/v1/workspaces";

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
                            assert_eq!(request, "device-session-v1\n");
                            let body = json!({
                                "origin": self.origin,
                                "access_token": KEY,
                                "personal_workspace_id": WORKSPACE
                            });
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
                 source_kind TEXT, source_label TEXT,
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
                "INSERT INTO session VALUES (?1, 'claude-code', ?2, ?3, 'explicit', ?4, 'file', ?5)",
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
    db.execute_batch(
        r#"ALTER TABLE session ADD COLUMN fingerprint_source_path TEXT;
           ALTER TABLE session ADD COLUMN source_format TEXT;"#,
    )
    .unwrap();
    db.execute(
        r#"INSERT INTO session VALUES ('native', 'opencode', 'sample', 'Select this', 'explicit', 30,
                'providerDb', 'opencode:sample:open_code_v2_sqlite',
                ?1, '"open_code_v2_sqlite"');"#,
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

fn document(session: &str, revision: &str, cursor: Option<&str>) -> String {
    let cursor = serde_json::to_string(&cursor).unwrap();
    http(
        "200 OK",
        &format!(
            r#"{{"workspace_id":"{WORKSPACE}","session_id":"{session}","revision":"{revision}","stale":true,"messages":[{{"role":"user","sequence":90071992547409931234567890}}],"page":{{"next_cursor":{cursor}}}}}"#
        ),
    )
}

#[test]
fn reads_one_page_and_keeps_the_exact_publication_and_message_values() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let server = serve_on(
        listener.try_clone().unwrap(),
        document(SESSION, REVISION, Some(CURSOR)),
    );
    let fixture = Fixture::new(&origin);
    let output = fixture.run(&["get", SESSION, "--source", "cloud"]);
    assert!(String::from_utf8_lossy(&output.stdout).contains("90071992547409931234567890"));
    let first = read_success(output);
    assert_eq!(first["session"]["cloud"]["id"], SESSION);
    assert!(first["session"].get("local").is_none());
    assert_eq!(first["document"]["revision"], REVISION);
    assert_eq!(first["document"]["stale"], true);
    assert_eq!(first["document"]["messages"][0]["role"], "user");
    assert_cloud_request(
        server,
        &format!("{ROUTE_PREFIX}/{WORKSPACE}/sessions/{SESSION}/document"),
    );

    let cursor = first["document"]["next_cursor"].as_str().unwrap();
    for (revision, expected) in [(REVISION, true), (OTHER, false)] {
        let server = serve_on(
            listener.try_clone().unwrap(),
            document(SESSION, revision, None),
        );
        let output = fixture.run(&["get", SESSION, "--source", "cloud", "--after", cursor]);
        if expected {
            assert!(read_success(output)["document"]["next_cursor"].is_null());
        } else {
            assert_private_error(output, "does not match the request");
        }
        assert_cloud_request(
            server,
            &format!(
                "{ROUTE_PREFIX}/{WORKSPACE}/sessions/{SESSION}/document?page%5Bafter%5D={CURSOR}"
            ),
        );
    }
    let other_connection = Fixture::new("http://127.0.0.1:1");
    assert_private_error(
        other_connection.run(&["get", SESSION, "--source", "cloud", "--after", cursor]),
        "invalid document cursor",
    );
}

#[test]
fn displays_one_cloud_document_page_in_text_mode() {
    let (origin, server) = serve(document(SESSION, REVISION, Some(CURSOR)));
    let fixture = Fixture::new(&origin);
    let output = fixture.run_connected(
        fixture
            .command()
            .args(["get", SESSION, "--source", "cloud"]),
    );
    assert!(output.status.success(), "{:?}", output.stderr);
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains(&format!("Cloud: {SESSION}")));
    assert!(text.contains(&format!("Document revision: {REVISION}")));
    assert!(text.contains("Messages on this page: 1"));
    let origin_key = URL_SAFE_NO_PAD.encode(Sha256::digest(&origin));
    assert!(text.contains(&format!(
        "Next page: --after {origin_key}.{WORKSPACE}.{SESSION}.{REVISION}.{CURSOR}"
    )));
    assert_cloud_request(
        server,
        &format!("{ROUTE_PREFIX}/{WORKSPACE}/sessions/{SESSION}/document"),
    );
}

#[test]
fn rejects_a_different_document() {
    let (origin, server) = serve(document(OTHER, REVISION, None));
    let fixture = Fixture::new(&origin);
    assert_private_error(
        fixture.run(&["get", SESSION, "--source", "cloud"]),
        "does not match the request",
    );
    assert_cloud_request(
        server,
        &format!("{ROUTE_PREFIX}/{WORKSPACE}/sessions/{SESSION}/document"),
    );
}

#[test]
fn restarts_without_the_cursor_after_publication_replacement() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let fixture = Fixture::new(&format!("http://{}", listener.local_addr().unwrap()));
    let route = format!("{ROUTE_PREFIX}/{WORKSPACE}/sessions/{SESSION}/document");
    let args = ["get", SESSION, "--source", "cloud"];

    let server = serve_on(
        listener.try_clone().unwrap(),
        document(SESSION, REVISION, Some(CURSOR)),
    );
    let first = read_success(fixture.run(&args));
    let cursor = first["document"]["next_cursor"].as_str().unwrap();
    assert_cloud_request(server, &route);

    let server = serve_on(
        listener.try_clone().unwrap(),
        http("409 Conflict", "private-synthetic-body"),
    );
    let mut continuation = args.to_vec();
    continuation.extend(["--after", cursor]);
    assert_private_error(fixture.run(&continuation), "Start again without --after");
    assert_cloud_request(server, &format!("{route}?page%5Bafter%5D={CURSOR}"));

    let server = serve_on(listener, document(SESSION, OTHER, None));
    let replacement = read_success(fixture.run(&args));
    assert_eq!(replacement["document"]["revision"], OTHER);
    assert_cloud_request(server, &route);
}

#[test]
fn rejects_overlong_cloud_requests_before_network_io() {
    let fixture = Fixture::new("http://127.0.0.1:1");
    for args in [
        vec!["search", "sample", "--source", "cloud", "--limit", "51"],
        vec!["search", &"a".repeat(257), "--source", "cloud"],
    ] {
        assert_private_error(fixture.run(&args), "Cloud search allows 1 to 50");
    }
    assert_private_error(
        fixture.run(&[
            "get",
            SESSION,
            "--source",
            "cloud",
            "--after",
            "private-synthetic-body",
        ]),
        "invalid document cursor",
    );
}

#[test]
fn rejects_an_oversized_success_body_without_showing_it() {
    let body = "private-synthetic-body".repeat(10_000);
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
