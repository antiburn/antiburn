// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Shell policy for the floating usage HUD.
//!
//! The `antiburn-hud` crate owns the window mechanism. This module keeps the
//! engine-specific activity lookup and its cost bound inside the shell.

use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Return the newest recent transcript write as epoch seconds.
pub async fn latest_session_activity() -> Option<i64> {
    static MEMO: Mutex<Option<(Instant, Option<i64>)>> = Mutex::new(None);
    const MEMO_TTL: Duration = Duration::from_secs(60);

    if let Ok(memo) = MEMO.lock()
        && let Some((at, value)) = *memo
        && at.elapsed() < MEMO_TTL
    {
        return value;
    }

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0);
    let logs = antiburn_local::discovery::Explorers::DISK
        .discover_recent_sessions(now, antiburn_local::discovery::ACTIVE_SESSION_WINDOW_SECS)
        .await;
    let latest = logs.iter().filter_map(|log| log.updated_at).max();

    if let Ok(mut memo) = MEMO.lock() {
        *memo = Some((Instant::now(), latest));
    }
    latest
}
