//! Guard the one-use browser callback with async socket reads and bounded timers.
//! The listener handles callback proof, not access/refresh tokens or keyring work.

use super::auth::{Attempt, Callback, Config, Failure, validate_callback};
use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener},
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    time::{Instant, sleep, sleep_until, timeout, timeout_at},
};

/// Own an ephemeral literal-loopback socket until callback, cancellation, or expiry.
pub struct Listener {
    listener: TcpListener,
    callback: String,
}

impl Listener {
    /// Bind before browser launch, from the admitted blocking auth operation.
    /// The nonblocking socket transfers to Tokio only when `wait` starts.
    pub fn bind() -> Result<Self, Failure> {
        let listener = TcpListener::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))
            .map_err(|_| Failure::Unavailable)?;
        listener
            .set_nonblocking(true)
            .map_err(|_| Failure::Unavailable)?;
        let callback = format!(
            "http://{}/cloud/callback",
            listener.local_addr().map_err(|_| Failure::Unavailable)?
        );
        Ok(Self { listener, callback })
    }

    /// Return the exact redirect URI that must bind the pending browser attempt.
    pub fn callback(&self) -> &str {
        &self.callback
    }

    /// Wait outside the controller lock and blocking-work slot. Poll the cheap
    /// generation predicate so cancellation does not require another socket.
    /// Require a Tokio runtime with I/O enabled. Each active request has bounded
    /// read/write waits; the caller must recheck generations before engine work.
    pub async fn wait(
        self,
        config: &Config,
        attempt: &Attempt,
        current: impl Fn() -> bool,
    ) -> Option<Callback> {
        let listener = tokio::net::TcpListener::from_std(self.listener).ok()?;
        let remaining = attempt
            .expires_at
            .saturating_sub(super::now())
            .clamp(0, 600) as u64;
        let deadline = Instant::now() + Duration::from_secs(remaining);
        while current() && Instant::now() < deadline {
            let accepted = tokio::select! {
                biased;
                _ = sleep_until(deadline) => return None,
                _ = sleep(Duration::from_millis(25)) => continue,
                accepted = listener.accept() => accepted,
            };
            match accepted {
                Ok((mut stream, peer)) if peer.ip().is_loopback() => {
                    let result = request(&mut stream).await.and_then(|(target, host)| {
                        validate_callback(&target, &host, config, attempt, super::now())
                    });
                    let status = if result.is_some() {
                        "200 OK"
                    } else {
                        "400 Bad Request"
                    };
                    let body = if result.is_some() {
                        "Return to Antiburn. You can close this page."
                    } else {
                        "This callback is not valid."
                    };
                    let response = format!(
                        "HTTP/1.1 {status}\r\nContent-Type: text/plain; charset=utf-8\r\nCache-Control: no-store\r\nReferrer-Policy: no-referrer\r\nContent-Security-Policy: default-src 'none'\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}",
                        body.len()
                    );
                    // A browser response failure must not repeat a valid callback.
                    let _ = timeout(
                        Duration::from_millis(250),
                        stream.write_all(response.as_bytes()),
                    )
                    .await;
                    if result.is_some() {
                        return result;
                    }
                }
                Ok(_) => {}
                Err(_) => return None,
            }
        }
        None
    }
}

/// Read one bounded HTTP/1.1 header block. Reject bodies and ambiguous routing
/// before returning the only values needed for callback proof validation.
async fn request(stream: &mut TcpStream) -> Option<(String, String)> {
    let deadline = Instant::now() + Duration::from_secs(1);
    let mut bytes = Vec::new();
    let mut buffer = [0u8; 1024];
    while bytes.len() <= 12_288 && Instant::now() < deadline {
        let read_deadline = deadline.min(Instant::now() + Duration::from_millis(250));
        let count = timeout_at(read_deadline, stream.read(&mut buffer))
            .await
            .ok()?
            .ok()?;
        if count == 0 {
            return None;
        }
        bytes.extend_from_slice(&buffer[..count]);
        if let Some(end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
            if end + 4 != bytes.len() {
                return None;
            }
            let headers = std::str::from_utf8(&bytes[..end]).ok()?;
            let mut lines = headers.split("\r\n");
            let parts = lines.next()?.split(' ').collect::<Vec<_>>();
            if parts.len() != 3 || parts[0] != "GET" || parts[2] != "HTTP/1.1" {
                return None;
            }
            let mut host = None;
            for line in lines {
                let (name, value) = line.split_once(':')?;
                if name.eq_ignore_ascii_case("host") {
                    if host.replace(value.trim().to_owned()).is_some() {
                        return None;
                    }
                } else if name.eq_ignore_ascii_case("transfer-encoding")
                    || (name.eq_ignore_ascii_case("content-length") && value.trim() != "0")
                    || name.eq_ignore_ascii_case("origin")
                {
                    return None;
                }
            }
            return Some((parts[1].to_owned(), host?));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    async fn pending_listener() -> (Listener, Attempt) {
        tokio::task::spawn_blocking(|| {
            let listener = Listener::bind().unwrap();
            let attempt = Attempt {
                callback: listener.callback().into(),
                state: "synthetic-state".into(),
                verifier: "synthetic-verifier".into(),
                epoch: "synthetic-epoch".into(),
                expires_at: super::super::now() + 600,
                generation: 0,
            };
            (listener, attempt)
        })
        .await
        .unwrap()
    }

    #[tokio::test(flavor = "current_thread")]
    async fn cloud_async_loopback_cancellation_does_not_need_a_connection_or_blocking_slot() {
        let (listener, attempt) = pending_listener().await;
        let active = Arc::new(AtomicBool::new(true));
        let waiting = active.clone();
        let task = tokio::spawn(async move {
            listener
                .wait(
                    &Config::parse("https://api.example", "https://api.example").unwrap(),
                    &attempt,
                    || waiting.load(Ordering::Acquire),
                )
                .await
        });
        tokio::task::yield_now().await;
        super::super::work::BlockingWork::default()
            .run(super::super::work::Priority::Normal, || Ok(()))
            .await
            .unwrap();
        assert!(!task.is_finished());
        active.store(false, Ordering::Release);
        assert!(
            timeout(Duration::from_secs(1), task)
                .await
                .unwrap()
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn cloud_async_loopback_expires_without_a_blocking_sleep() {
        let (listener, attempt) = pending_listener().await;
        let task = tokio::spawn(async move {
            listener
                .wait(
                    &Config::parse("https://api.example", "https://api.example").unwrap(),
                    &attempt,
                    || true,
                )
                .await
        });
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_secs(601)).await;
        assert!(task.await.unwrap().is_none());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn cloud_async_loopback_bounds_a_stalled_header_and_still_cancels() {
        let (listener, attempt) = pending_listener().await;
        let address = attempt
            .callback
            .strip_prefix("http://")
            .unwrap()
            .strip_suffix("/cloud/callback")
            .unwrap()
            .to_owned();
        let active = Arc::new(AtomicBool::new(true));
        let waiting = active.clone();
        let task = tokio::spawn(async move {
            listener
                .wait(
                    &Config::parse("https://api.example", "https://api.example").unwrap(),
                    &attempt,
                    || waiting.load(Ordering::Acquire),
                )
                .await
        });
        let mut stream = TcpStream::connect(address).await.unwrap();
        stream.write_all(b"GET /cloud/callback?").await.unwrap();
        let mut reply = String::new();
        timeout(Duration::from_secs(2), stream.read_to_string(&mut reply))
            .await
            .unwrap()
            .unwrap();
        assert!(reply.starts_with("HTTP/1.1 400"));
        active.store(false, Ordering::Release);
        assert!(
            timeout(Duration::from_secs(1), task)
                .await
                .unwrap()
                .unwrap()
                .is_none()
        );
    }
}
