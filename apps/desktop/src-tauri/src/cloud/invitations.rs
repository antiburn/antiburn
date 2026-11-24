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
const LIST_ADMISSION_WAIT: Duration = Duration::from_millis(750);
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
    Start {
        invitation_id: Option<String>,
        workspace_name: Option<String>,
        workspace_slug: Option<String>,
    },
    Status {
        id: String,
    },
    Finish {
        id: String,
    },
    Cancel {
        id: String,
    },
    Dismiss {
        id: String,
    },
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

impl State {
    fn is_terminal(self) -> bool {
        !matches!(self, Self::Waiting | Self::Ready)
    }
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
        workspace_name: Option<String>,
        workspace_slug: Option<String>,
    },
    Uncertain {
        id: String,
        join: bool,
    },
    Expired {
        uncertain: bool,
    },
    ReauthRequired {
        uncertain: bool,
    },
}

/// Keep an attempt and its result in memory for one account generation.
/// A renderer can reopen after its previous request already completed.
pub(super) struct Pending {
    id: String,
    generation: u64,
    join: bool,
    workspace_name: Option<String>,
    workspace_slug: Option<String>,
    deadline: Instant,
    state: State,
    workspace_id: Option<String>,
    uncertain: bool,
}

impl Pending {
    fn expired(&self) -> bool {
        Instant::now() >= self.deadline
    }

    fn reply(&self) -> Reply {
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
            workspace_id: self.workspace_id.clone(),
            workspace_name: self.workspace_name.clone(),
            workspace_slug: self.workspace_slug.clone(),
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
    // The signed-in event can reach the panel before the callback frees the slot.
    let priority = if matches!(request, Action::List) {
        Priority::Read(LIST_ADMISSION_WAIT)
    } else {
        Priority::Normal
    };
    work.run(priority, move || {
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
        let status = engine.status;
        let result =
            engine.with_invitation_access(&account_id, current, |config, bearer, can_send| {
                let device = DeviceRequest {
                    config,
                    bearer,
                    can_send,
                };
                run_action(&mut pending, generation, request, &device, |url| {
                    app.opener()
                        .open_url(url.as_str(), None::<&str>)
                        .map_err(|_| Failure::Unavailable)
                })
            });
        let reauth_required = current() && matches!(result, Err(Failure::ReauthRequired));
        let uncertain = pending
            .as_ref()
            .filter(|attempt| attempt.generation == generation && attempt.uncertain);
        let warn = reauth_required && uncertain.is_some_and(|attempt| attempt.join);
        let warning_changed = warn && !engine.invitation_result_unconfirmed;
        engine.invitation_result_unconfirmed |= warn;
        let uncertain = uncertain.is_some();
        drop(pending);
        if current() && (engine.status != status || warning_changed) {
            let snapshot = super::snapshot(&app, &engine);
            if current() {
                super::emit_snapshot(&app, snapshot);
            }
        }
        match result {
            Err(Failure::ReauthRequired) if current() => Ok(Reply::ReauthRequired { uncertain }),
            result => result.map_err(|_| "Could not complete the Cloud invitation request.".into()),
        }
    })
    .await
}

fn run_action(
    pending: &mut Option<Pending>,
    generation: u64,
    request: Action,
    device: &DeviceRequest<'_>,
    open: impl FnOnce(Url) -> Result<(), Failure>,
) -> Result<Reply, Failure> {
    if pending
        .as_ref()
        .is_some_and(|attempt| attempt.generation != generation)
    {
        *pending = None;
    }
    match request {
        Action::List => {
            if let Some(attempt) = pending {
                if attempt.state.is_terminal() || !attempt.expired() {
                    return Ok(attempt.reply());
                }
                let uncertain = attempt.uncertain;
                *pending = None;
                if uncertain {
                    return Ok(Reply::Expired { uncertain });
                }
            }
            Ok(Reply::List {
                listing: list(device)?,
            })
        }
        Action::Dismiss { id } => {
            if !pending
                .as_ref()
                .is_some_and(|attempt| attempt.id == id && attempt.state.is_terminal())
            {
                return Err(Failure::Cancelled);
            }
            let listing = list(device)?;
            *pending = None;
            Ok(Reply::List { listing })
        }
        Action::Start {
            invitation_id,
            workspace_name,
            workspace_slug,
        } => {
            if let Some(attempt) = pending
                && !attempt.state.is_terminal()
                && !attempt.expired()
            {
                return Ok(attempt.reply());
            }
            if invitation_id
                .as_ref()
                .is_some_and(|id| !valid_label(id, 256))
            {
                return Err(Failure::InvalidResponse);
            }
            let deadline = Instant::now() + VERIFY_WINDOW;
            let body = serde_json::json!({ "invitation_id": invitation_id });
            let created: Created =
                device.send(Method::POST, ATTEMPTS_PATH, Some(body), Some(deadline))?;
            attempt_path(&created.id)?;
            let url = browser_url(device.config, &created.browser_url)?;
            if !(device.can_send)() || Instant::now() >= deadline {
                return Err(Failure::Cancelled);
            }
            open(url)?;
            let attempt = Pending {
                id: created.id,
                generation,
                join: invitation_id.is_some(),
                // Display labels cannot change the invitation ID sent to the service.
                workspace_name: workspace_name
                    .filter(|name| invitation_id.is_some() && valid_workspace_name(name)),
                workspace_slug: workspace_slug
                    .filter(|slug| invitation_id.is_some() && valid_label(slug, 256)),
                deadline,
                state: State::Waiting,
                workspace_id: None,
                uncertain: false,
            };
            let reply = attempt.reply();
            *pending = Some(attempt);
            Ok(reply)
        }
        Action::Status { ref id } | Action::Finish { ref id } | Action::Cancel { ref id } => {
            update(pending, &request, id, device)
        }
    }
}

fn update(
    pending: &mut Option<Pending>,
    request: &Action,
    id: &str,
    device: &DeviceRequest<'_>,
) -> Result<Reply, Failure> {
    let attempt = pending
        .as_mut()
        .filter(|attempt| attempt.id == id)
        .ok_or(Failure::Cancelled)?;
    if attempt.state.is_terminal() {
        return match request {
            Action::Status { .. } => Ok(attempt.reply()),
            _ => Err(Failure::Cancelled),
        };
    }
    if attempt.expired() {
        let uncertain = attempt.uncertain;
        *pending = None;
        return Ok(Reply::Expired { uncertain });
    }
    let path = attempt_path(&attempt.id)?;
    let deadline = Some(attempt.deadline);
    let observe = || device.send(Method::GET, &path, None, deadline);
    let mutation = match request {
        Action::Finish { .. } if attempt.can_finish() => Some("finish"),
        Action::Cancel { .. } if !attempt.uncertain => Some("cancel"),
        Action::Status { .. } => None,
        _ => return Err(Failure::Cancelled),
    };
    let observed = if let Some(action) = mutation {
        attempt.uncertain = true;
        device
            .send(Method::POST, &path, Some(action.into()), deadline)
            .or_else(|error| match error {
                Failure::ReauthRequired => Err(error),
                _ => observe(),
            })
    } else {
        observe()
    };
    let status: Status = match observed {
        Ok(status) => status,
        Err(Failure::ReauthRequired) => return Err(Failure::ReauthRequired),
        Err(_) if attempt.expired() => {
            let uncertain = attempt.uncertain;
            *pending = None;
            return Ok(Reply::Expired { uncertain });
        }
        Err(_) if mutation.is_some() => return Ok(attempt.reply()),
        Err(error) => return Err(error),
    };
    attempt.state = status.state;
    attempt.uncertain = false;
    attempt.workspace_id = status.workspace_id;
    if !attempt.state.is_terminal() && attempt.expired() {
        *pending = None;
        return Ok(Reply::Expired { uncertain: false });
    }
    Ok(attempt.reply())
}

fn list(device: &DeviceRequest<'_>) -> Result<Listing, Failure> {
    let listing = match device.send(Method::GET, LIST_PATH, None, None) {
        Err(Failure::Unavailable) => return Ok(Listing::Unavailable),
        result => result?,
    };
    if let Listing::Checked { invitations } = &listing
        && invitations.iter().any(|entry| {
            !valid_label(&entry.id, 256)
                || !valid_workspace_name(&entry.workspace_name)
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

fn valid_workspace_name(value: &str) -> bool {
    value.chars().take(201).count() <= 200
        && !value.trim().is_empty()
        && !value.chars().any(char::is_control)
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

/// Bind request dispatch to fresh account authority, including fallback reads.
struct DeviceRequest<'a> {
    config: &'a Config,
    bearer: &'a str,
    can_send: &'a dyn Fn() -> bool,
}

impl DeviceRequest<'_> {
    /// Use the existing origin and redirect rules. A timed attempt must not
    /// send after its deadline or wait longer than its remaining time.
    fn send<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        body: Option<serde_json::Value>,
        deadline: Option<Instant>,
    ) -> Result<T, Failure> {
        let url = endpoint(self.config, path)?;
        let mut request = auth::client(&url)?
            .request(method, url)
            .bearer_auth(self.bearer);
        if let Some(body) = body {
            request = request.json(&body);
        }
        if let Some(deadline) = deadline {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(Failure::Cancelled);
            }
            request = request.timeout(remaining.min(Duration::from_secs(15)));
        }
        if !(self.can_send)() {
            return Err(Failure::Cancelled);
        }
        let response = request.send().map_err(|_| Failure::Offline)?;
        match response.status() {
            status if status.is_success() => auth::decode_json(response),
            StatusCode::NOT_FOUND | StatusCode::METHOD_NOT_ALLOWED => Err(Failure::Unavailable),
            StatusCode::UNAUTHORIZED => Err(Failure::ReauthRequired),
            StatusCode::FORBIDDEN => Err(Failure::Rejected),
            _ => Err(Failure::Offline),
        }
    }
}

#[cfg(all(test, debug_assertions))]
#[path = "tests/invitations_tests.rs"]
mod tests;
