//! Account-bound device invitation requests. Browser tickets stay in Rust.

use super::{
    Controller,
    auth::{self, Config, Failure},
    work::Priority,
};
use reqwest::{Method, StatusCode};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::{
    sync::atomic::Ordering,
    time::{Duration, Instant},
};
use tauri::Manager;
use tauri_plugin_opener::OpenerExt;
use url::Url;

const VERIFY_PATH: &str = "/devices/invitations/verify";
const LIST_PATH: &str = "/v1/devices/invitations";
const ATTEMPTS_PATH: &str = "/v1/devices/invitation-attempts";
const VERIFY_WINDOW: Duration = Duration::from_secs(5 * 60);

/// The legacy desktop grant cannot act on device invitation routes.
pub(super) fn uses_account_route(config: &Config) -> bool {
    config.authorization.path() == "/devices/authorize/account"
}

/// Probe the configured API in development builds only. An unsupported or
/// unreachable capability endpoint must not prevent normal browser sign-in.
pub(super) fn supported(config: &Config) -> bool {
    #[derive(Deserialize)]
    struct Capabilities {
        version: u64,
    }
    cfg!(debug_assertions)
        && uses_account_route(config)
        && endpoint(config, "/v1/devices/invitation-capabilities")
            .and_then(|url| {
                let response = auth::client(&url)?
                    .get(url)
                    .timeout(Duration::from_secs(3))
                    .send()
                    .map_err(|_| Failure::Offline)?;
                if response.status() != StatusCode::OK {
                    return Err(Failure::Unavailable);
                }
                auth::decode_json::<Capabilities>(response)
            })
            .is_ok_and(|capabilities| capabilities.version == 1)
}

/// IPC takes actions and account bindings, not URLs, tickets, or bearer proof.
#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Action {
    List,
    Start { invitation_id: Option<String> },
    Status { id: String },
    Finish { id: String },
    Cancel { id: String },
}

/// List entries are display data. A workspace slug does not authorize joining.
#[derive(Deserialize, Serialize)]
pub(crate) struct Invitation {
    id: String,
    workspace_name: String,
    workspace_slug: String,
}

/// Keep an unchecked or unavailable list distinct from a verified empty list.
#[derive(Deserialize, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub(crate) enum Listing {
    Checked { invitations: Vec<Invitation> },
    CheckRequired,
    Unavailable,
}

#[derive(Deserialize)]
struct Created {
    id: String,
    browser_url: String,
}

/// Service attempt states. Only Ready can lead to an explicit Desktop finish.
#[derive(Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum State {
    Waiting,
    Ready,
    Checked,
    Unavailable,
    WrongAccount,
    Canceled,
    Joined,
    NoSeat,
    Closed,
}

#[derive(Deserialize)]
struct Status {
    state: State,
    workspace_id: Option<String>,
}

/// Safe renderer results. `Uncertain` means query this attempt, not start a
/// replacement. `Expired` is a local deadline, not a claim about the server.
#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum Reply {
    List {
        listing: Listing,
    },
    Attempt {
        id: String,
        join: bool,
        state: State,
        workspace_id: Option<String>,
    },
    Uncertain {
        id: String,
        join: bool,
    },
    Expired,
}

/// Memory-only attempt owned by one account generation. Keep a potentially
/// committed finish attached to the same attempt after a lost reply.
pub(super) struct Pending {
    id: String,
    generation: u64,
    join: bool,
    deadline: Instant,
    state: State,
    uncertain: bool,
}

impl Pending {
    fn expired(&self) -> bool {
        Instant::now() >= self.deadline
    }

    fn reply(&self, workspace_id: Option<String>) -> Reply {
        if self.uncertain {
            return Reply::Uncertain {
                id: self.id.clone(),
                join: self.join,
            };
        }
        Reply::Attempt {
            id: self.id.clone(),
            join: self.join,
            state: self.state,
            workspace_id,
        }
    }

    fn can_finish(&self) -> bool {
        self.state == State::Ready && !self.uncertain && !self.expired()
    }
}

/// Run one short request in the existing bounded worker. Poll sleeps belong to
/// the renderer and never hold this worker, the attempt lock, or the account lock.
#[tauri::command]
pub(crate) async fn cloud_invitations(
    app: tauri::AppHandle,
    account_id: String,
    request: Action,
) -> Result<Reply, String> {
    let controller = app.state::<Controller>();
    let generation = controller.account_generation.load(Ordering::Acquire);
    let work = controller.blocking.clone();
    work.run(Priority::Normal, move || {
        let controller = app.state::<Controller>();
        let current = || controller.account_generation.load(Ordering::Acquire) == generation;
        let mut engine = controller
            .engine
            .lock()
            .map_err(|_| "Could not open Cloud invitations.")?;
        if engine.generation != generation {
            return Err("Could not open Cloud invitations.".into());
        }
        let mut pending = controller
            .invitations
            .lock()
            .map_err(|_| "Could not open Cloud invitations.")?;
        if pending
            .as_ref()
            .is_some_and(|attempt| attempt.generation != generation)
        {
            *pending = None;
        }
        engine
            .with_invitation_access(&account_id, current, |config, bearer| {
                run_action(&mut pending, generation, request, config, bearer, |url| {
                    if !current() {
                        return Err(Failure::Cancelled);
                    }
                    app.opener()
                        .open_url(url.as_str(), None::<&str>)
                        .map_err(|_| Failure::Unavailable)
                })
            })
            .map_err(|_| "Could not complete the Cloud invitation request.".into())
    })
    .await
}

fn run_action(
    pending: &mut Option<Pending>,
    generation: u64,
    request: Action,
    config: &Config,
    bearer: &str,
    open: impl FnOnce(Url) -> Result<(), Failure>,
) -> Result<Reply, Failure> {
    match request {
        Action::List => {
            if let Some(attempt) = pending {
                if !attempt.expired() || attempt.uncertain {
                    return Ok(attempt.reply(None));
                }
                *pending = None;
            }
            let listing = list(config, bearer)?;
            Ok(Reply::List { listing })
        }
        Action::Start { invitation_id } => {
            if let Some(attempt) = pending
                && (!attempt.expired() || attempt.uncertain)
            {
                return Ok(attempt.reply(None));
            }
            if invitation_id
                .as_ref()
                .is_some_and(|id| !valid_label(id, 256))
            {
                return Err(Failure::InvalidResponse);
            }
            let deadline = Instant::now() + VERIFY_WINDOW;
            let body = serde_json::json!({ "invitation_id": invitation_id });
            let created: Created = send(config, bearer, Method::POST, ATTEMPTS_PATH, Some(body))?;
            attempt_path(&created.id)?;
            open(browser_url(config, &created.browser_url)?)?;
            let attempt = Pending {
                id: created.id,
                generation,
                join: invitation_id.is_some(),
                deadline,
                state: State::Waiting,
                uncertain: false,
            };
            let reply = attempt.reply(None);
            *pending = Some(attempt);
            Ok(reply)
        }
        Action::Status { ref id } | Action::Finish { ref id } | Action::Cancel { ref id } => {
            update(pending, &request, id, config, bearer)
        }
    }
}

fn update(
    pending: &mut Option<Pending>,
    request: &Action,
    id: &str,
    config: &Config,
    bearer: &str,
) -> Result<Reply, Failure> {
    let attempt = pending
        .as_mut()
        .filter(|attempt| attempt.id == id)
        .ok_or(Failure::Cancelled)?;
    if attempt.expired() && !attempt.uncertain && !matches!(request, Action::Cancel { .. }) {
        *pending = None;
        return Ok(Reply::Expired);
    }
    let path = attempt_path(&attempt.id)?;
    let observe = || send(config, bearer, Method::GET, &path, None);
    let mutation = match request {
        Action::Finish { .. } if attempt.can_finish() => Some("finish"),
        Action::Cancel { .. } if !attempt.uncertain => Some("cancel"),
        Action::Status { .. } => None,
        _ => return Err(Failure::Cancelled),
    };
    let status: Status = if let Some(action) = mutation {
        attempt.uncertain = true;
        match send(config, bearer, Method::POST, &path, Some(action.into())).or_else(|_| observe())
        {
            Ok(status) => status,
            Err(_) => return Ok(attempt.reply(None)),
        }
    } else {
        observe()?
    };
    attempt.state = status.state;
    attempt.uncertain = false;
    let reply = attempt.reply(status.workspace_id);
    if !matches!(status.state, State::Waiting | State::Ready) {
        *pending = None;
    } else if attempt.expired() {
        *pending = None;
        return Ok(Reply::Expired);
    }
    Ok(reply)
}

fn list(config: &Config, bearer: &str) -> Result<Listing, Failure> {
    let listing = match send(config, bearer, Method::GET, LIST_PATH, None) {
        Err(Failure::Unavailable) => return Ok(Listing::Unavailable),
        result => result?,
    };
    if let Listing::Checked { invitations } = &listing
        && invitations.iter().any(|entry| {
            !valid_label(&entry.id, 256)
                || !valid_label(&entry.workspace_name, 256)
                || !valid_label(&entry.workspace_slug, 256)
        })
    {
        return Err(Failure::InvalidResponse);
    }
    Ok(listing)
}

fn valid_label(value: &str, max: usize) -> bool {
    !value.trim().is_empty() && value.len() <= max && !value.chars().any(char::is_control)
}

fn attempt_path(id: &str) -> Result<String, Failure> {
    if id.is_empty()
        || id.len() > 256
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(Failure::InvalidResponse);
    }
    Ok(format!("{ATTEMPTS_PATH}/{id}"))
}

fn endpoint(config: &Config, path: &str) -> Result<Url, Failure> {
    Url::parse(&format!("{}{path}", config.resource)).map_err(|_| Failure::Unavailable)
}

/// Accept the exact canonical API URL and one nonempty opaque ticket. Checking
/// the raw prefix also rejects aliases that the URL parser could normalize.
fn browser_url(config: &Config, raw: &str) -> Result<Url, Failure> {
    let prefix = format!("{}{VERIFY_PATH}?ticket=", config.resource);
    if raw.len() > 8192
        || !raw.starts_with(&prefix)
        || raw.contains('&')
        || raw.chars().any(char::is_whitespace)
        || raw.chars().any(char::is_control)
    {
        return Err(Failure::InvalidResponse);
    }
    let url = Url::parse(raw).map_err(|_| Failure::InvalidResponse)?;
    let mut pairs = url.query_pairs();
    let valid_ticket = pairs
        .next()
        .is_some_and(|(key, value)| key == "ticket" && valid_label(&value, 4096))
        && pairs.next().is_none();
    if url.path() != VERIFY_PATH || url.fragment().is_some() || !valid_ticket {
        return Err(Failure::InvalidResponse);
    }
    Ok(url)
}

/// Use the same timeout, origin rules, and no-redirect policy as account proof.
fn send<T: DeserializeOwned>(
    config: &Config,
    bearer: &str,
    method: Method,
    path: &str,
    body: Option<serde_json::Value>,
) -> Result<T, Failure> {
    let url = endpoint(config, path)?;
    let request = auth::client(&url)?.request(method, url).bearer_auth(bearer);
    let request = match body {
        Some(body) => request.json(&body),
        None => request,
    };
    let response = request.send().map_err(|_| Failure::Offline)?;
    match response.status() {
        status if status.is_success() => auth::decode_json(response),
        StatusCode::NOT_FOUND | StatusCode::METHOD_NOT_ALLOWED => Err(Failure::Unavailable),
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => Err(Failure::Rejected),
        _ => Err(Failure::Offline),
    }
}

#[cfg(all(test, debug_assertions))]
#[path = "tests/invitations_tests.rs"]
mod tests;
