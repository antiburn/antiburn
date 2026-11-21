use super::*;
use std::{
    io::{BufRead, BufReader, Read, Write},
    net::TcpListener,
    thread::JoinHandle,
};

struct Request {
    line: String,
    headers: String,
    body: String,
}

fn device(config: &Config) -> DeviceRequest<'_> {
    DeviceRequest {
        config,
        bearer: "test-device",
        can_send: &|| true,
    }
}

/// Serve one response per connection. `None` drops the reply after reading the request.
fn serve(
    replies: impl FnOnce(&str) -> Vec<Option<(u16, String)>>,
) -> (Config, JoinHandle<Vec<Request>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let resource = format!("http://{}", listener.local_addr().unwrap());
    let mut config = Config::parse("https://issuer.example", &resource).unwrap();
    config.authorization = endpoint(&config, "/devices/authorize/account").unwrap();
    let replies = replies(&resource);
    let worker = std::thread::spawn(move || {
        replies
            .into_iter()
            .map(|reply| {
                let (mut stream, _) = listener.accept().unwrap();
                stream.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
                let mut reader = BufReader::new(&mut stream);
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                let mut headers = String::new();
                loop {
                    let mut header = String::new();
                    assert!(reader.read_line(&mut header).unwrap() > 0);
                    if header == "\r\n" {
                        break;
                    }
                    headers.push_str(&header.to_ascii_lowercase());
                }
                let length = headers
                    .lines()
                    .find_map(|line| line.strip_prefix("content-length: "))
                    .map_or(0, |length| length.parse().unwrap());
                let mut body = vec![0; length];
                reader.read_exact(&mut body).unwrap();
                if let Some((status, body)) = reply {
                    write!(
                        stream,
                        "HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    )
                    .unwrap();
                }
                Request {
                    line: line.trim_end().into(),
                    headers,
                    body: String::from_utf8(body).unwrap(),
                }
            })
            .collect()
    });
    (config, worker)
}

#[test]
fn capability_version_one_alone_opts_in_at_account_sign_in() {
    use super::super::{engine::Engine, state::StateStore, tests::MemoryVault};
    use std::sync::Arc;

    for (body, expected) in [
        (r#"{"version":1}"#, true),
        (r#"{"version":2}"#, false),
        (r#"{"version":"1"}"#, false),
        ("{}", false),
    ] {
        let (config, worker) = serve(|_| vec![Some((200, body.into()))]);
        let directory = tempfile::tempdir().unwrap();
        let mut engine = Engine::new(
            Some(config),
            Box::new(Arc::new(MemoryVault::default())),
            Box::new(auth::HttpTransport),
            StateStore::new(directory.path()),
        );
        let (url, _, _) = engine.begin(1_000, 1).unwrap();
        let url = Url::parse(&url).unwrap();
        assert_eq!(url.path(), "/devices/authorize/account");
        assert_eq!(
            url.query_pairs()
                .any(|(key, value)| key == "native_invitations" && value == "true"),
            expected
        );
        let requests = worker.join().unwrap();
        assert_eq!(
            requests[0].line,
            "GET /v1/devices/invitation-capabilities HTTP/1.1"
        );
        assert!(!requests[0].headers.contains("authorization:"));
        assert!(!requests[0].headers.contains("cookie:"));
        assert!(!requests[0].headers.contains("origin:"));
    }
}

#[test]
fn invitation_list_distinguishes_checked_empty_from_needing_browser_proof() {
    let (config, worker) = serve(|_| {
        vec![
            Some((200, r#"{"state":"check_required"}"#.into())),
            Some((200, r#"{"state":"checked","invitations":[]}"#.into())),
            Some((404, "{}".into())),
        ]
    });
    assert!(matches!(
        list(&device(&config)).unwrap(),
        Listing::CheckRequired
    ));
    assert!(matches!(
        list(&device(&config)).unwrap(),
        Listing::Checked { invitations } if invitations.is_empty()
    ));
    assert!(matches!(
        list(&device(&config)).unwrap(),
        Listing::Unavailable
    ));
    for request in worker.join().unwrap() {
        assert_eq!(request.line, "GET /v1/devices/invitations HTTP/1.1");
        assert!(
            request
                .headers
                .contains("authorization: bearer test-device")
        );
        assert!(request.body.is_empty());
    }
}

#[test]
fn browser_open_requires_exact_api_url_and_one_ticket() {
    let config = Config::parse("https://issuer.example", "https://api.example").unwrap();
    assert!(
        browser_url(
            &config,
            "https://api.example/devices/invitations/verify?ticket=opaque"
        )
        .is_ok()
    );
    for url in [
        "https://issuer.example/devices/invitations/verify?ticket=opaque",
        "https://api.example:443/devices/invitations/verify?ticket=opaque",
        "https://user@api.example/devices/invitations/verify?ticket=opaque",
        "https://api.example/devices/invitations/../invitations/verify?ticket=opaque",
        "https://api.example/devices/invitations/%76erify?ticket=opaque",
        "https://api.example/devices/invitations/verify?ticket=",
        "https://api.example/devices/invitations/verify?ticket=x&ticket=y",
        "https://api.example/devices/invitations/verify?ticket=x&next=y",
        "https://api.example/devices/invitations/verify?ticket=x#fragment",
        "https://api.example/devices/invitations/verify?ticket=%0d",
    ] {
        assert!(browser_url(&config, url).is_err(), "{url}");
    }
    for id in ["", "..", "a/b", "a?b", "a%2fb"] {
        assert!(attempt_path(id).is_err(), "{id}");
    }
}

#[test]
fn finish_requires_ready_and_recovers_a_lost_reply_on_the_same_attempt() {
    let (config, worker) = serve(|origin| {
        vec![
            Some((
                200,
                serde_json::json!({
                    "id": "attempt-1",
                    "browser_url": format!("{origin}{VERIFY_PATH}?ticket=test")
                })
                .to_string(),
            )),
            Some((200, r#"{"state":"ready","workspace_id":null}"#.into())),
            None,
            Some((200, r#"{"state":"joined","workspace_id":"team-1"}"#.into())),
            Some((200, r#"{"state":"checked","invitations":[]}"#.into())),
        ]
    });
    let mut pending = None;
    let mut send = |request| {
        run_action(&mut pending, 1, request, &device(&config), |url| {
            assert_eq!(url.path(), VERIFY_PATH);
            Ok(())
        })
    };
    let id = "attempt-1".to_owned();
    send(Action::Start {
        invitation_id: Some("invitation-1".into()),
        workspace_name: Some("Example Team".into()),
    })
    .unwrap();
    assert!(matches!(
        send(Action::Finish { id: id.clone() }),
        Err(Failure::Cancelled)
    ));
    send(Action::Status { id: id.clone() }).unwrap();
    assert!(matches!(
        send(Action::Finish { id: id.clone() }).unwrap(),
        Reply::Attempt {
            state: State::Joined,
            workspace_id: Some(workspace_id),
            workspace_name: Some(workspace_name),
            ..
        } if workspace_id == "team-1" && workspace_name == "Example Team"
    ));
    assert!(matches!(
        send(Action::List).unwrap(),
        Reply::Attempt {
            state: State::Joined,
            workspace_id: Some(workspace_id),
            ..
        } if workspace_id == "team-1"
    ));
    assert!(matches!(
        send(Action::Status { id: id.clone() }).unwrap(),
        Reply::Attempt {
            state: State::Joined,
            ..
        }
    ));
    assert!(matches!(
        send(Action::Finish { id: id.clone() }),
        Err(Failure::Cancelled)
    ));
    assert!(matches!(
        send(Action::Dismiss { id }).unwrap(),
        Reply::List {
            listing: Listing::Checked { .. }
        }
    ));
    assert!(pending.is_none());
    let requests = worker.join().unwrap();
    assert_eq!(
        requests[0].line,
        "POST /v1/devices/invitation-attempts HTTP/1.1"
    );
    assert_eq!(requests[0].body, r#"{"invitation_id":"invitation-1"}"#);
    assert_eq!(
        requests[1].line,
        "GET /v1/devices/invitation-attempts/attempt-1 HTTP/1.1"
    );
    assert_eq!(
        requests[2].line,
        "POST /v1/devices/invitation-attempts/attempt-1 HTTP/1.1"
    );
    assert_eq!(requests[2].body, r#""finish""#);
    assert_eq!(requests[3].line, requests[1].line);
    assert_eq!(requests[4].line, "GET /v1/devices/invitations HTTP/1.1");
    for request in requests {
        assert!(
            request
                .headers
                .contains("authorization: bearer test-device")
        );
        assert!(!request.headers.contains("cookie:"));
        assert!(!request.headers.contains("origin:"));
    }
}

#[test]
fn check_attempt_sends_null_and_cancellation_sends_a_json_string() {
    let (config, worker) = serve(|origin| {
        vec![
            Some((
                201,
                serde_json::json!({
                    "id": "check-1",
                    "browser_url": format!("{origin}{VERIFY_PATH}?ticket=test")
                })
                .to_string(),
            )),
            Some((200, r#"{"state":"canceled","workspace_id":null}"#.into())),
        ]
    });
    let mut pending = None;
    let mut send = |request| run_action(&mut pending, 1, request, &device(&config), |_| Ok(()));
    send(Action::Start {
        invitation_id: None,
        workspace_name: None,
    })
    .unwrap();
    assert!(matches!(
        send(Action::Cancel {
            id: "check-1".into()
        })
        .unwrap(),
        Reply::Attempt {
            state: State::Canceled,
            join: false,
            ..
        }
    ));
    let requests = worker.join().unwrap();
    assert_eq!(requests[0].body, r#"{"invitation_id":null}"#);
    assert_eq!(
        requests[1].line,
        "POST /v1/devices/invitation-attempts/check-1 HTTP/1.1"
    );
    assert_eq!(requests[1].body, r#""cancel""#);
}

#[test]
fn finish_and_cancel_fence_the_fallback_status_request() {
    use std::cell::Cell;

    for (request, body) in [
        (
            Action::Finish {
                id: "attempt-1".into(),
            },
            r#""finish""#,
        ),
        (
            Action::Cancel {
                id: "attempt-1".into(),
            },
            r#""cancel""#,
        ),
    ] {
        let (config, worker) = serve(|_| vec![None]);
        let checks = Cell::new(0);
        let mut device = device(&config);
        let can_send = || {
            checks.set(checks.get() + 1);
            checks.get() == 1
        };
        device.can_send = &can_send;
        let mut pending = Some(ready_attempt(Instant::now() + VERIFY_WINDOW));
        let reply = update(&mut pending, &request, "attempt-1", &device).unwrap();
        assert!(matches!(reply, Reply::Uncertain { .. }));
        assert_eq!(checks.get(), 2, "fallback must check authority again");
        let requests = worker.join().unwrap();
        assert_eq!(requests[0].body, body);
    }
}

#[test]
fn an_expired_uncertain_attempt_does_not_send_another_status_request() {
    let config = Config::parse("https://issuer.example", "https://api.example").unwrap();
    let mut device = device(&config);
    device.can_send = &|| panic!("an expired attempt must not reach dispatch");
    let id = "attempt-1";
    let mut attempt = ready_attempt(Instant::now());
    attempt.uncertain = true;
    let mut pending = Some(attempt);
    assert!(matches!(
        update(&mut pending, &Action::Status { id: id.into() }, id, &device),
        Ok(Reply::Expired { uncertain: true })
    ));
    assert!(pending.is_none());
    assert!(matches!(
        device.send::<Status>(Method::GET, LIST_PATH, None, Some(Instant::now())),
        Err(Failure::Cancelled)
    ));
}

#[test]
fn a_completed_result_does_not_cross_account_generations() {
    let (config, worker) = serve(|_| {
        vec![Some((
            200,
            r#"{"state":"checked","invitations":[]}"#.into(),
        ))]
    });
    let mut attempt = ready_attempt(Instant::now());
    attempt.state = State::Joined;
    attempt.workspace_id = Some("team-1".into());
    let mut pending = Some(attempt);
    let device = device(&config);
    assert!(matches!(
        run_action(&mut pending, 1, Action::List, &device, |_| Ok(())),
        Ok(Reply::Attempt {
            state: State::Joined,
            ..
        })
    ));
    assert!(matches!(
        run_action(&mut pending, 2, Action::List, &device, |_| Ok(())),
        Ok(Reply::List { .. })
    ));
    assert!(pending.is_none());
    assert_eq!(
        worker.join().unwrap()[0].line,
        "GET /v1/devices/invitations HTTP/1.1"
    );
}

fn ready_attempt(deadline: Instant) -> Pending {
    Pending {
        id: "attempt-1".into(),
        generation: 1,
        join: true,
        workspace_name: Some("Example Team".into()),
        deadline,
        state: State::Ready,
        workspace_id: None,
        uncertain: false,
    }
}
