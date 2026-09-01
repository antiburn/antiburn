//! Bounded local Antigravity and `agy` language-server fallback.
//!
//! Process discovery accepts only known Antigravity executable markers. Port
//! discovery accepts only explicit extension ports or PID-owned loopback
//! listeners. Every list and command output has a fixed cap.

use std::collections::{HashMap, HashSet};
use std::io::Read as _;
use std::process::Stdio;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde_json::json;
use time::OffsetDateTime;

use crate::provider_usage::live::antigravity;
use crate::provider_usage::live::model::{
    Confidence, Freshness, ProviderUsageError, ProviderUsageSnapshot, UsageSource,
};

use super::http;

const STATUS_PATH: &str = "/exa.language_server_pb.LanguageServerService/GetUserStatus";
const SUMMARY_PATH: &str = "/exa.language_server_pb.LanguageServerService/RetrieveUserQuotaSummary";
const MAX_COMMAND_BYTES: usize = 512 * 1024;
const MAX_CANDIDATES: usize = 8;
const MAX_PORTS_PER_CANDIDATE: usize = 8;
const MAX_TOTAL_PROBES: usize = 24;
const PROBE_BUDGET: Duration = Duration::from_secs(8);
const COMMAND_TIMEOUT: Duration = Duration::from_secs(2);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(3);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProcessKind {
    Agy,
    Ide,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Candidate {
    pid: u32,
    kind: ProcessKind,
    csrf: Option<String>,
    extension_port: Option<u16>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum LoopbackHost {
    V4,
    V6,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct Listener {
    host: LoopbackHost,
    port: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Scheme {
    Http,
    Https,
}

pub(super) trait LocalUsageTransport: Send + Sync {
    fn fetch(
        &self,
        now: OffsetDateTime,
    ) -> Result<Option<ProviderUsageSnapshot>, ProviderUsageError>;
}

pub(super) struct LocalProbe;

#[derive(Clone)]
enum LocalReply {
    Body(String),
    Unsupported,
}

trait LocalEndpointTransport {
    fn request(
        &self,
        listener: Listener,
        scheme: Scheme,
        csrf: Option<&str>,
        path: &str,
    ) -> Result<LocalReply, ProviderUsageError>;
}

struct HttpEndpointTransport;

impl LocalEndpointTransport for HttpEndpointTransport {
    fn request(
        &self,
        listener: Listener,
        scheme: Scheme,
        csrf: Option<&str>,
        path: &str,
    ) -> Result<LocalReply, ProviderUsageError> {
        request_local(listener, scheme, csrf, path)
    }
}

impl LocalUsageTransport for LocalProbe {
    fn fetch(
        &self,
        now: OffsetDateTime,
    ) -> Result<Option<ProviderUsageSnapshot>, ProviderUsageError> {
        let candidates = discover_candidates();
        if candidates.is_empty() {
            return Ok(None);
        }
        let listeners = discover_listeners(&candidates);
        let deadline = Instant::now() + PROBE_BUDGET;
        let mut probes = 0;
        let mut last_error = None;
        for candidate in candidates {
            let endpoints = candidate_endpoints(&candidate, listeners.get(&candidate.pid));
            let mut seen = HashSet::new();
            for (listener, scheme) in endpoints {
                if probes >= MAX_TOTAL_PROBES || Instant::now() >= deadline {
                    break;
                }
                if !seen.insert((listener, scheme)) {
                    continue;
                }
                if probes + 2 > MAX_TOTAL_PROBES {
                    break;
                }
                probes += 2;
                match request_local_pair(
                    &HttpEndpointTransport,
                    listener,
                    scheme,
                    candidate.csrf.as_deref(),
                ) {
                    Ok((summary, status)) => {
                        let windows = antigravity::merge_windows(
                            summary.map_or_else(Vec::new, |summary| summary.windows),
                            status.windows,
                        );
                        return Ok(Some(ProviderUsageSnapshot {
                            provider: crate::provider_usage::providers::GOOGLE,
                            account: status.account,
                            plan: status.plan,
                            plan_tier: status.tier,
                            observed_at: now,
                            source: UsageSource {
                                id: super::ANTIGRAVITY_SOURCE_ID,
                                label: match candidate.kind {
                                    ProcessKind::Agy => "Read from the Antigravity CLI",
                                    ProcessKind::Ide => "Read from Antigravity IDE",
                                }
                                .into(),
                                confidence: Confidence::Medium,
                                freshness: Freshness::Fresh,
                            },
                            windows,
                            supplemental: None,
                            reset_credits: None,
                        }));
                    }
                    Err(error) => last_error = Some(preferred_error(last_error, error)),
                }
            }
        }
        Err(last_error.unwrap_or(ProviderUsageError::Unavailable))
    }
}

fn request_local_pair(
    transport: &dyn LocalEndpointTransport,
    listener: Listener,
    scheme: Scheme,
    csrf: Option<&str>,
) -> Result<(Option<antigravity::QuotaSummary>, antigravity::LocalStatus), ProviderUsageError> {
    let summary = transport.request(listener, scheme, csrf, SUMMARY_PATH);
    let status = transport.request(listener, scheme, csrf, STATUS_PATH);
    let status = match status? {
        LocalReply::Body(body) => antigravity::parse_get_user_status(&body)?,
        LocalReply::Unsupported => return Err(ProviderUsageError::Unavailable),
    };
    let summary = match summary? {
        LocalReply::Body(body) => Some(antigravity::parse_quota_summary(&body)?),
        LocalReply::Unsupported => None,
    };
    Ok((summary, status))
}

fn candidate_endpoints(
    candidate: &Candidate,
    listeners: Option<&Vec<Listener>>,
) -> Vec<(Listener, Scheme)> {
    let mut endpoints = Vec::new();
    for listener in listeners.into_iter().flatten() {
        if candidate.extension_port == Some(listener.port) {
            endpoints.push((*listener, Scheme::Http));
        }
        endpoints.push((*listener, Scheme::Https));
        endpoints.push((*listener, Scheme::Http));
    }
    endpoints
}

fn preferred_error(
    current: Option<ProviderUsageError>,
    candidate: ProviderUsageError,
) -> ProviderUsageError {
    let rank = |error| match error {
        ProviderUsageError::Authentication => 4,
        ProviderUsageError::RateLimited => 3,
        ProviderUsageError::Schema(_) => 2,
        ProviderUsageError::Unavailable => 1,
    };
    match current {
        Some(current) if rank(current) >= rank(candidate) => current,
        _ => candidate,
    }
}

fn local_client() -> &'static reqwest::blocking::Client {
    static CLIENT: std::sync::OnceLock<reqwest::blocking::Client> = std::sync::OnceLock::new();
    CLIENT
        .get_or_init(|| build_local_client().expect("the local client uses no custom TLS material"))
}

fn build_local_client() -> Result<reqwest::blocking::Client, reqwest::Error> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    // This client accepts local self-signed certificates only. Its URL
    // builder accepts a loopback enum and it has no OAuth-token input.
    reqwest::blocking::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(REQUEST_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .danger_accept_invalid_certs(true)
        .build()
}

fn request_local(
    listener: Listener,
    scheme: Scheme,
    csrf: Option<&str>,
    path: &str,
) -> Result<LocalReply, ProviderUsageError> {
    let scheme = match scheme {
        Scheme::Http => "http",
        Scheme::Https => "https",
    };
    let host = match listener.host {
        LoopbackHost::V4 => "127.0.0.1".to_owned(),
        LoopbackHost::V6 => "[::1]".to_owned(),
    };
    let url = format!("{scheme}://{host}:{}{path}", listener.port);
    let mut request = local_client()
        .post(url)
        .header(reqwest::header::ACCEPT, "application/json")
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .header("Connect-Protocol-Version", "1")
        .json(&json!({
            "metadata": {
                "ideName": "antigravity",
                "extensionName": "antigravity",
                "ideVersion": "unknown",
                "locale": "en"
            }
        }));
    if let Some(csrf) = csrf.filter(|csrf| !csrf.is_empty()) {
        request = request.header("X-Codeium-Csrf-Token", csrf);
    }
    let response = request
        .send()
        .map_err(|_| ProviderUsageError::Unavailable)?;
    if is_unsupported_rpc(response.status()) {
        return Ok(LocalReply::Unsupported);
    }
    check_local_status(response.status())?;
    http::read_capped_body(response).map(LocalReply::Body)
}

fn is_unsupported_rpc(status: reqwest::StatusCode) -> bool {
    matches!(
        status,
        reqwest::StatusCode::NOT_FOUND
            | reqwest::StatusCode::METHOD_NOT_ALLOWED
            | reqwest::StatusCode::NOT_IMPLEMENTED
    )
}

fn check_local_status(status: reqwest::StatusCode) -> Result<(), ProviderUsageError> {
    match http::status_error(status) {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

fn discover_candidates() -> Vec<Candidate> {
    let Some(executables) = process_executable_list() else {
        return Vec::new();
    };
    let arguments = process_argument_list().unwrap_or_default();
    parse_processes(&executables, &arguments)
}

#[cfg(target_os = "linux")]
fn process_executable_list() -> Option<String> {
    run_bounded("ps", &["-ww", "-ax", "-o", "pid=", "-o", "exe="])
}

#[cfg(all(unix, not(target_os = "linux")))]
fn process_executable_list() -> Option<String> {
    run_bounded("ps", &["-ww", "-ax", "-o", "pid=", "-o", "comm="])
}

#[cfg(unix)]
fn process_argument_list() -> Option<String> {
    run_bounded("ps", &["-ww", "-ax", "-o", "pid=", "-o", "args="])
}

#[cfg(windows)]
fn process_executable_list() -> Option<String> {
    run_bounded(
        "powershell.exe",
        &[
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "Get-CimInstance Win32_Process | ForEach-Object { \"$($_.ProcessId)`t$($_.ExecutablePath)\" }",
        ],
    )
}

#[cfg(windows)]
fn process_argument_list() -> Option<String> {
    run_bounded(
        "powershell.exe",
        &[
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "Get-CimInstance Win32_Process | ForEach-Object { \"$($_.ProcessId)`t$($_.CommandLine)\" }",
        ],
    )
}

#[cfg(not(any(unix, windows)))]
fn process_executable_list() -> Option<String> {
    None
}

#[cfg(not(any(unix, windows)))]
fn process_argument_list() -> Option<String> {
    None
}

fn parse_processes(executables: &str, arguments: &str) -> Vec<Candidate> {
    let argument_map: HashMap<u32, &str> = arguments.lines().filter_map(parse_pid_value).collect();
    let mut candidates = Vec::new();
    let mut seen = HashSet::new();
    for (pid, executable) in executables.lines().filter_map(parse_pid_value) {
        let Some(kind) = classify_executable(executable) else {
            continue;
        };
        if !seen.insert(pid) {
            continue;
        }
        let command = argument_map.get(&pid).copied().unwrap_or_default();
        candidates.push(Candidate {
            pid,
            kind,
            csrf: flag_value(command, "--csrf_token"),
            extension_port: flag_value(command, "--extension_server_port")
                .and_then(|port| port.parse::<u16>().ok())
                .filter(|port| *port != 0),
        });
        if candidates.len() == MAX_CANDIDATES {
            break;
        }
    }
    candidates
}

fn parse_pid_value(line: &str) -> Option<(u32, &str)> {
    let line = line.trim();
    let split = line.find(char::is_whitespace)?;
    let pid = line[..split].parse().ok()?;
    let value = line[split..].trim();
    (!value.is_empty()).then_some((pid, value))
}

fn classify_executable(executable: &str) -> Option<ProcessKind> {
    const PRODUCT_EXECUTABLES: [&str; 7] = [
        "language_server_macos_arm",
        "language_server_macos_x64",
        "language_server_macos",
        "language_server_linux_arm64",
        "language_server_linux_x64",
        "language_server_linux",
        "language_server_windows.exe",
    ];
    let executable = executable.trim_matches(['"', '\'']);
    let basename = executable.rsplit(['/', '\\']).next()?.to_ascii_lowercase();
    if matches!(basename.as_str(), "agy" | "agy.exe") {
        return Some(ProcessKind::Agy);
    }
    if PRODUCT_EXECUTABLES.contains(&basename.as_str()) && antigravity_path(executable) {
        return Some(ProcessKind::Ide);
    }
    if matches!(
        basename.as_str(),
        "language_server" | "language_server.exe" | "language-server"
    ) && antigravity_path(executable)
    {
        return Some(ProcessKind::Ide);
    }
    None
}

fn antigravity_path(executable: &str) -> bool {
    executable.split(['/', '\\']).any(|component| {
        matches!(
            component.to_ascii_lowercase().as_str(),
            "antigravity"
                | "antigravity ide"
                | "antigravity-ide"
                | "antigravity.app"
                | "antigravity ide.app"
        )
    })
}

fn flag_value(command: &str, flag: &str) -> Option<String> {
    let mut parts = command.split_whitespace();
    while let Some(part) = parts.next() {
        let part = part.trim_matches(['"', '\'']);
        if part == flag {
            return parts
                .next()
                .map(|value| value.trim_matches(['"', '\'']).to_owned())
                .filter(|value| !value.is_empty());
        }
        if let Some(value) = part
            .strip_prefix(flag)
            .and_then(|rest| rest.strip_prefix('='))
        {
            let value = value.trim_matches(['"', '\'']);
            if !value.is_empty() {
                return Some(value.to_owned());
            }
        }
    }
    None
}

fn discover_listeners(candidates: &[Candidate]) -> HashMap<u32, Vec<Listener>> {
    if candidates.is_empty() {
        return HashMap::new();
    }
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        let pids = candidates
            .iter()
            .map(|candidate| candidate.pid.to_string())
            .collect::<Vec<_>>()
            .join(",");
        if let Some(output) =
            run_bounded("lsof", &["-nP", "-a", "-p", &pids, "-iTCP", "-sTCP:LISTEN"])
        {
            return parse_lsof(&output, candidates);
        }
        #[cfg(target_os = "linux")]
        if let Some(output) = run_bounded("ss", &["-ltnp"]) {
            return parse_ss(&output, candidates);
        }
    }
    #[cfg(target_os = "windows")]
    if let Some(output) = run_bounded("netstat.exe", &["-ano", "-p", "tcp"]) {
        return parse_netstat(&output, candidates);
    }
    HashMap::new()
}

#[cfg(any(target_os = "macos", target_os = "linux", test))]
fn parse_lsof(output: &str, candidates: &[Candidate]) -> HashMap<u32, Vec<Listener>> {
    let allowed: HashSet<u32> = candidates.iter().map(|candidate| candidate.pid).collect();
    let mut found = HashMap::new();
    for line in output.lines().skip(1) {
        let columns: Vec<&str> = line.split_whitespace().collect();
        let Some(pid) = columns.get(1).and_then(|value| value.parse::<u32>().ok()) else {
            continue;
        };
        if !allowed.contains(&pid) || !line.contains("(LISTEN)") {
            continue;
        }
        if let Some(listener) = columns.iter().find_map(|value| parse_loopback(value)) {
            push_listener(&mut found, pid, listener);
        }
    }
    found
}

#[cfg(target_os = "linux")]
fn parse_ss(output: &str, candidates: &[Candidate]) -> HashMap<u32, Vec<Listener>> {
    let mut found = HashMap::new();
    for candidate in candidates {
        for line in output
            .lines()
            .filter(|line| ss_line_has_pid(line, candidate.pid))
        {
            if let Some(listener) = line.split_whitespace().find_map(parse_loopback) {
                push_listener(&mut found, candidate.pid, listener);
            }
        }
    }
    found
}

#[cfg(any(target_os = "linux", test))]
fn ss_line_has_pid(line: &str, wanted: u32) -> bool {
    let mut rest = line;
    while let Some(index) = rest.find("pid=") {
        rest = &rest[index + 4..];
        let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
        if digits != 0 && rest[..digits].parse::<u32>().ok() == Some(wanted) {
            return true;
        }
        rest = &rest[digits..];
    }
    false
}

#[cfg(target_os = "windows")]
fn parse_netstat(output: &str, candidates: &[Candidate]) -> HashMap<u32, Vec<Listener>> {
    let allowed: HashSet<u32> = candidates.iter().map(|candidate| candidate.pid).collect();
    let mut found = HashMap::new();
    for line in output.lines().filter(|line| line.contains("LISTENING")) {
        let columns: Vec<&str> = line.split_whitespace().collect();
        let Some(pid) = columns.last().and_then(|value| value.parse::<u32>().ok()) else {
            continue;
        };
        if !allowed.contains(&pid) {
            continue;
        }
        if let Some(listener) = columns.get(1).and_then(|value| parse_loopback(value)) {
            push_listener(&mut found, pid, listener);
        }
    }
    found
}

fn push_listener(found: &mut HashMap<u32, Vec<Listener>>, pid: u32, listener: Listener) {
    let listeners = found.entry(pid).or_default();
    if listeners.len() < MAX_PORTS_PER_CANDIDATE && !listeners.contains(&listener) {
        listeners.push(listener);
    }
}

fn parse_loopback(value: &str) -> Option<Listener> {
    let value = value.trim_matches([',', '(', ')']);
    let (host, port) = value.rsplit_once(':')?;
    let port = port.parse::<u16>().ok().filter(|port| *port != 0)?;
    let host = host.trim_matches(['[', ']']);
    let host = match host {
        "127.0.0.1" | "localhost" => LoopbackHost::V4,
        "::1" => LoopbackHost::V6,
        _ => return None,
    };
    Some(Listener { host, port })
}

fn run_bounded(program: &str, args: &[&str]) -> Option<String> {
    let mut child = antiburn_local::platform::process::headless_std_command(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let stdout = child.stdout.take()?;
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let result = stdout
            .take(MAX_COMMAND_BYTES as u64 + 1)
            .read_to_end(&mut bytes);
        let _ = tx.send((result, bytes));
    });
    let (result, bytes) = match rx.recv_timeout(COMMAND_TIMEOUT) {
        Ok(output) => output,
        Err(_) => {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
    };
    let status = child.wait().ok()?;
    if result.is_err() || !status.success() || bytes.len() > MAX_COMMAND_BYTES {
        return None;
    }
    String::from_utf8(bytes).ok()
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;

    const SUMMARY: &str = r#"{"groups":[
      {"displayName":"Gemini","buckets":[
        {"bucketId":"gemini-5h","remainingFraction":0.8},
        {"bucketId":"gemini-weekly","remainingFraction":0.7}]},
      {"displayName":"Claude + GPT","buckets":[
        {"bucketId":"3p-5h","remainingFraction":0.6},
        {"bucketId":"3p-weekly","remainingFraction":0.5}]}
    ]}"#;
    const STATUS: &str = r#"{"userStatus":{
      "email":"person@example.test",
      "planStatus":{"planInfo":{"planName":"Pro","planTier":"pro-tier"}},
      "clientModelConfigs":[
        {"label":"Gemini 3 Pro","quotaInfo":{"remainingFraction":0.4}}
      ]
    }}"#;

    struct FakeEndpoint {
        summary: Result<LocalReply, ProviderUsageError>,
        calls: RefCell<Vec<String>>,
    }

    impl LocalEndpointTransport for FakeEndpoint {
        fn request(
            &self,
            _: Listener,
            _: Scheme,
            _: Option<&str>,
            path: &str,
        ) -> Result<LocalReply, ProviderUsageError> {
            self.calls.borrow_mut().push(path.to_owned());
            if path == SUMMARY_PATH {
                self.summary.clone()
            } else {
                Ok(LocalReply::Body(STATUS.to_owned()))
            }
        }
    }

    fn local_pair(
        fake: &FakeEndpoint,
    ) -> (Option<antigravity::QuotaSummary>, antigravity::LocalStatus) {
        request_local_pair(
            fake,
            Listener {
                host: LoopbackHost::V4,
                port: 3210,
            },
            Scheme::Http,
            Some("synthetic-csrf"),
        )
        .unwrap()
    }

    #[test]
    fn local_pair_uses_summary_windows_and_status_metadata() {
        let fake = FakeEndpoint {
            summary: Ok(LocalReply::Body(SUMMARY.to_owned())),
            calls: RefCell::default(),
        };
        let (summary, status) = local_pair(&fake);
        assert_eq!(summary.unwrap().windows.len(), 4);
        assert_eq!(status.account.as_deref(), Some("person@example.test"));
        assert_eq!(status.plan.as_deref(), Some("Pro"));
        assert_eq!(fake.calls.borrow().as_slice(), [SUMMARY_PATH, STATUS_PATH]);
    }

    #[test]
    fn local_shared_windows_hide_model_fallback_detail() {
        let fake = FakeEndpoint {
            summary: Ok(LocalReply::Body(SUMMARY.to_owned())),
            calls: RefCell::default(),
        };
        let (summary, status) = local_pair(&fake);
        let windows = antigravity::merge_windows(summary.unwrap().windows, status.windows);
        assert_eq!(windows.len(), 4);
        assert!(windows.iter().any(|window| window.id.ends_with("weekly")));
        assert!(windows.iter().all(|window| !window.id.contains("-model-")));
    }

    #[test]
    fn local_pair_falls_back_to_status_windows_when_summary_is_unsupported() {
        let fake = FakeEndpoint {
            summary: Ok(LocalReply::Unsupported),
            calls: RefCell::default(),
        };
        let (summary, status) = local_pair(&fake);
        assert!(summary.is_none());
        assert_eq!(status.windows.len(), 1);
        assert_eq!(fake.calls.borrow().as_slice(), [SUMMARY_PATH, STATUS_PATH]);
    }

    #[test]
    fn local_pair_rejects_a_malformed_supported_summary_after_reading_status() {
        let fake = FakeEndpoint {
            summary: Ok(LocalReply::Body("{}".to_owned())),
            calls: RefCell::default(),
        };
        let result = request_local_pair(
            &fake,
            Listener {
                host: LoopbackHost::V4,
                port: 3210,
            },
            Scheme::Http,
            None,
        );
        assert!(matches!(result, Err(ProviderUsageError::Schema(_))));
        assert_eq!(fake.calls.borrow().as_slice(), [SUMMARY_PATH, STATUS_PATH]);
    }

    #[test]
    fn process_parser_accepts_current_names_flags_and_bounds_candidates() {
        let mut executables = String::from(
            "10 /Applications/Antigravity IDE.app/bin/language_server_macos_arm\n\
             11 /home/a/antigravity/bin/language_server_linux_x64\n\
             12 C:\\Antigravity\\language_server_windows.exe\n\
             13 /usr/local/bin/agy\n",
        );
        let mut arguments = String::from(
            "10 language_server_macos_arm --csrf_token csrf-a --extension_server_port=3210\n\
             11 language_server_linux_x64 --csrf_token=csrf-b\n\
             12 language_server_windows.exe --extension_server_port 4321\n\
             13 agy\n",
        );
        for pid in 20..40 {
            executables.push_str(&format!("{pid} /usr/local/bin/agy\n"));
            arguments.push_str(&format!("{pid} agy\n"));
        }
        let parsed = parse_processes(&executables, &arguments);
        assert_eq!(parsed.len(), MAX_CANDIDATES);
        assert_eq!(parsed[0].extension_port, Some(3210));
        assert_eq!(parsed[0].csrf.as_deref(), Some("csrf-a"));
        assert_eq!(parsed[1].kind, ProcessKind::Ide);
        assert_eq!(parsed[2].extension_port, Some(4321));
        assert_eq!(parsed[3].kind, ProcessKind::Agy);
    }

    #[test]
    fn process_identity_rejects_command_line_and_path_spoofs() {
        let executables = "10 /bin/bash\n\
                           11 /usr/local/bin/agy-helper\n\
                           12 /tmp/not-antigravity/language_server\n\
                           13 /Applications/Antigravity IDE.app/bin/language_server\n\
                           14 /Applications/Windsurf.app/bin/language_server_macos_arm\n";
        let arguments = "10 bash -c /usr/local/bin/agy --extension_server_port 9999\n\
                         11 agy-helper --csrf_token spoof\n\
                         12 language_server --app_data_dir antigravity\n\
                         13 language_server --csrf_token real\n\
                         14 language_server_macos_arm --csrf_token other-product\n";
        let parsed = parse_processes(executables, arguments);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].pid, 13);
        assert_eq!(parsed[0].csrf.as_deref(), Some("real"));
    }

    #[test]
    fn listener_parser_keeps_only_pid_owned_loopback_ports_and_bounds_each_pid() {
        let candidates = vec![Candidate {
            pid: 10,
            kind: ProcessKind::Agy,
            csrf: None,
            extension_port: None,
        }];
        let mut output = String::from("COMMAND PID USER FD TYPE DEVICE SIZE/OFF NODE NAME\n");
        output.push_str("agy 10 user 1u IPv4 x 0t0 TCP 127.0.0.1:3000 (LISTEN)\n");
        output.push_str("agy 10 user 1u IPv4 x 0t0 TCP 0.0.0.0:3001 (LISTEN)\n");
        output.push_str("agy 99 user 1u IPv4 x 0t0 TCP 127.0.0.1:3002 (LISTEN)\n");
        for port in 4000..4020 {
            output.push_str(&format!(
                "agy 10 user 1u IPv6 x 0t0 TCP [::1]:{port} (LISTEN)\n"
            ));
        }
        let found = parse_lsof(&output, &candidates);
        let ports = &found[&10];
        assert_eq!(ports.len(), MAX_PORTS_PER_CANDIDATE);
        assert!(ports.contains(&Listener {
            host: LoopbackHost::V4,
            port: 3000
        }));
        assert!(ports.iter().all(|listener| listener.port != 3001));
    }

    #[test]
    fn loopback_parser_rejects_wildcard_and_remote_addresses() {
        assert!(parse_loopback("127.0.0.1:1234").is_some());
        assert!(parse_loopback("[::1]:1234").is_some());
        assert!(parse_loopback("0.0.0.0:1234").is_none());
        assert!(parse_loopback("192.0.2.1:1234").is_none());
        assert!(parse_loopback("[::]:1234").is_none());
    }

    #[test]
    fn flags_require_valid_nonzero_ports() {
        let parsed = parse_processes(
            "1 /usr/bin/agy\n2 /usr/bin/agy\n",
            "1 agy --extension_server_port 0\n2 agy --extension_server_port 70000\n",
        );
        assert!(
            parsed
                .iter()
                .all(|candidate| candidate.extension_port.is_none())
        );
    }

    #[test]
    fn explicit_ports_must_be_owned_loopback_listeners() {
        let candidate = Candidate {
            pid: 10,
            kind: ProcessKind::Ide,
            csrf: Some("csrf".into()),
            extension_port: Some(3210),
        };
        let unrelated = vec![Listener {
            host: LoopbackHost::V4,
            port: 9999,
        }];
        let endpoints = candidate_endpoints(&candidate, Some(&unrelated));
        assert!(endpoints.iter().all(|(listener, _)| listener.port != 3210));

        let owned = vec![Listener {
            host: LoopbackHost::V4,
            port: 3210,
        }];
        let endpoints = candidate_endpoints(&candidate, Some(&owned));
        assert_eq!(endpoints[0], (owned[0], Scheme::Http));
    }

    #[test]
    fn ss_pid_matching_is_numeric_and_exact() {
        let line = r#"LISTEN 0 128 127.0.0.1:3210 0.0.0.0:* users:(("agy",pid=123,fd=7))"#;
        assert!(ss_line_has_pid(line, 123));
        assert!(!ss_line_has_pid(line, 12));
    }

    #[test]
    fn dedicated_loopback_client_builder_is_available() {
        // `build_local_client` contains the fixed no-proxy and no-redirect
        // policy. This seam keeps those settings separate from cloud clients.
        assert!(build_local_client().is_ok());
    }

    #[test]
    fn local_http_statuses_use_the_shared_error_taxonomy() {
        assert_eq!(check_local_status(reqwest::StatusCode::OK), Ok(()));
        assert_eq!(
            check_local_status(reqwest::StatusCode::UNAUTHORIZED),
            Err(ProviderUsageError::Authentication)
        );
        assert_eq!(
            check_local_status(reqwest::StatusCode::TOO_MANY_REQUESTS),
            Err(ProviderUsageError::RateLimited)
        );
        assert_eq!(
            check_local_status(reqwest::StatusCode::INTERNAL_SERVER_ERROR),
            Err(ProviderUsageError::Unavailable)
        );
        assert!(is_unsupported_rpc(reqwest::StatusCode::NOT_FOUND));
        assert!(is_unsupported_rpc(reqwest::StatusCode::METHOD_NOT_ALLOWED));
        assert!(is_unsupported_rpc(reqwest::StatusCode::NOT_IMPLEMENTED));
        assert!(!is_unsupported_rpc(
            reqwest::StatusCode::INTERNAL_SERVER_ERROR
        ));
    }
}
