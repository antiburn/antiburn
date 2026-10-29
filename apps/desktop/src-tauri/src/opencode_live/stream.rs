//! Opens and reads the bounded OpenCode V2 event stream.

use std::{
    fmt,
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
    time::Duration,
};

use reqwest::Response;

use super::{
    DatabaseAssociationError, RegistrationError, ServiceInfo, ServiceRegistration, SseDecoder,
    SseError, SseEvent, read_registration,
};

const MAX_SSE_FRAME_OVERHEAD_BYTES: usize = 1024;
const MAX_SSE_FRAME_BYTES: usize =
    antiburn_local::analysis::opencode_v2_capture::MAX_CAPTURE_EVENT_BYTES
        + MAX_SSE_FRAME_OVERHEAD_BYTES;
const MAX_BUFFERED_BYTES: usize = 1024 * 1024;
const MAX_DECODE_BYTES_PER_TURN: usize = 64 * 1024;
const MAX_EVENTS_PER_TURN: usize = 64;
const READ_IDLE_TIMEOUT: Duration = Duration::from_secs(45);
const CANCEL_POLL_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Clone, Copy)]
pub(crate) struct StreamTiming {
    pub idle_timeout: Duration,
    pub cancel_poll: Duration,
}

impl Default for StreamTiming {
    fn default() -> Self {
        Self {
            idle_timeout: READ_IDLE_TIMEOUT,
            cancel_poll: CANCEL_POLL_INTERVAL,
        }
    }
}

#[derive(Debug)]
pub enum EventStreamError {
    Registration(RegistrationError),
    Association(DatabaseAssociationError),
    Request,
    Redirect,
    HttpStatus(u16),
    NotEventStream,
    RegistrationChanged,
    Body,
    StreamEnded,
    Sse(SseError),
    CapacityExceeded,
    Cancelled,
    IdleTimeout,
}

impl fmt::Display for EventStreamError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Registration(_) => {
                formatter.write_str("OpenCode service registration is unavailable")
            }
            Self::Association(_) => {
                formatter.write_str("OpenCode service is not proven to use this database")
            }
            Self::Request => formatter.write_str("OpenCode event stream request failed"),
            Self::Redirect => formatter.write_str("OpenCode event stream returned a redirect"),
            Self::NotEventStream => {
                formatter.write_str("OpenCode service did not return an event stream")
            }
            Self::RegistrationChanged => {
                formatter.write_str("OpenCode service registration changed during connection")
            }
            Self::Body => formatter.write_str("OpenCode event stream ended with a read error"),
            Self::StreamEnded => formatter.write_str("OpenCode event stream ended"),
            Self::CapacityExceeded => {
                formatter.write_str("OpenCode event stream exceeded its buffer limit")
            }
            Self::Cancelled => formatter.write_str("OpenCode event stream was cancelled"),
            Self::IdleTimeout => formatter.write_str("OpenCode event stream heartbeat timed out"),
            Self::HttpStatus(status) => {
                write!(formatter, "OpenCode event stream returned HTTP {status}")
            }
            Self::Sse(error) => {
                write!(
                    formatter,
                    "OpenCode event stream frame was invalid: {error:?}"
                )
            }
        }
    }
}

impl std::error::Error for EventStreamError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Registration(error) => Some(error),
            Self::Association(error) => Some(error),
            _ => None,
        }
    }
}

/// An authenticated event stream bound to the selected native database.
///
/// The server does not replay events. A new stream is a new live boundary, and
/// an ended stream is a continuity gap. Callers must not use
/// `server.connected` as a history watermark.
pub struct EventStream {
    response: Option<Response>,
    decoder: SseDecoder,
    buffered: Vec<u8>,
    buffered_offset: usize,
    decoded_bytes: usize,
    decoded_events: usize,
    registration: ServiceRegistration,
    service_info: ServiceInfo,
    timing: StreamTiming,
    last_byte_at: tokio::time::Instant,
}

impl EventStream {
    pub fn registration(&self) -> &ServiceRegistration {
        &self.registration
    }

    pub fn service_info(&self) -> &ServiceInfo {
        &self.service_info
    }

    /// Return one event before decoding later frames. Callers can reject events
    /// before they enter a queue. Ended feeds and malformed frames fail.
    pub async fn next_event(
        &mut self,
        cancelled: &AtomicBool,
    ) -> Result<SseEvent, EventStreamError> {
        loop {
            if cancelled.load(std::sync::atomic::Ordering::SeqCst) {
                self.decoder.cancel();
                self.buffered.clear();
                self.response.take();
                return Err(EventStreamError::Cancelled);
            }
            if self.decoded_bytes >= MAX_DECODE_BYTES_PER_TURN
                || self.decoded_events >= MAX_EVENTS_PER_TURN
            {
                self.decoded_bytes = 0;
                self.decoded_events = 0;
                tokio::task::yield_now().await;
                continue;
            }
            if self.buffered_offset < self.buffered.len() {
                let start = self.buffered_offset;
                let end = self
                    .buffered
                    .len()
                    .min(start + MAX_DECODE_BYTES_PER_TURN - self.decoded_bytes);
                let mut remaining = &self.buffered[start..end];
                let event = self.decoder.next_event(&mut remaining);
                let consumed = end - start - remaining.len();
                self.buffered_offset += consumed;
                self.decoded_bytes += consumed;
                match event {
                    Some(Ok(event)) => {
                        self.decoded_events += 1;
                        return Ok(event);
                    }
                    Some(Err(error)) => {
                        self.decoder.cancel();
                        self.buffered.clear();
                        self.response.take();
                        return Err(EventStreamError::Sse(error));
                    }
                    None => continue,
                }
            }

            let Some(response) = self.response.as_mut() else {
                return Err(EventStreamError::StreamEnded);
            };
            let result = tokio::select! {
                result = response.chunk() => result,
                () = tokio::time::sleep_until(self.last_byte_at + self.timing.idle_timeout) => {
                    self.response.take();
                    return Err(EventStreamError::IdleTimeout);
                }
                () = tokio::time::sleep(self.timing.cancel_poll) => continue,
            };
            let chunk = match result {
                Ok(Some(chunk)) => chunk,
                Ok(None) => {
                    self.response.take();
                    return Err(EventStreamError::StreamEnded);
                }
                Err(_) => {
                    self.response.take();
                    return Err(EventStreamError::Body);
                }
            };
            self.last_byte_at = tokio::time::Instant::now();
            if chunk.len() > MAX_BUFFERED_BYTES {
                self.decoder.cancel();
                self.buffered.clear();
                self.response.take();
                return Err(EventStreamError::CapacityExceeded);
            }
            self.buffered = chunk.to_vec();
            self.buffered_offset = 0;
        }
    }
}

/// Connect to `/api/event` only after proving that the registered process has
/// the selected regular SQLite file open. Recheck the registration and file
/// binding after the stream response opens.
pub(crate) async fn open_verified_event_stream(
    registration_path: &Path,
    database_path: &Path,
    connect_timeout: Duration,
    timing: StreamTiming,
) -> Result<EventStream, EventStreamError> {
    let original = read_registration(registration_path).map_err(EventStreamError::Registration)?;
    let registration_for_proof = registration_path.to_path_buf();
    let database_for_proof = database_path.to_path_buf();
    let first_info =
        verify_binding(registration_for_proof, database_for_proof, connect_timeout).await?;
    let current = read_registration(registration_path).map_err(EventStreamError::Registration)?;
    if current != original
        || first_info.pid != current.pid
        || current
            .version
            .as_ref()
            .is_some_and(|version| version != &first_info.version)
    {
        return Err(EventStreamError::RegistrationChanged);
    }

    let client = super::local_client()
        .connect_timeout(connect_timeout)
        .read_timeout(timing.idle_timeout)
        .build()
        .map_err(|_| EventStreamError::Request)?;
    let request = super::authorized_get(&current.endpoint, &client, "/api/event")
        .map_err(|_| EventStreamError::Request)?;
    let response = tokio::time::timeout(connect_timeout, request.send())
        .await
        .map_err(|_| EventStreamError::Request)?
        .map_err(|_| EventStreamError::Request)?;
    if response.status().is_redirection() {
        return Err(EventStreamError::Redirect);
    }
    if !response.status().is_success() {
        return Err(EventStreamError::HttpStatus(response.status().as_u16()));
    }
    let is_event_stream = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value.split(';').next().is_some_and(|media_type| {
                media_type.trim().eq_ignore_ascii_case("text/event-stream")
            })
        });
    if !is_event_stream {
        return Err(EventStreamError::NotEventStream);
    }

    let after_info = verify_binding(
        registration_path.to_path_buf(),
        database_path.to_path_buf(),
        connect_timeout,
    )
    .await?;
    let after = read_registration(registration_path).map_err(EventStreamError::Registration)?;
    if after != current || after_info != first_info {
        return Err(EventStreamError::RegistrationChanged);
    }

    Ok(EventStream {
        response: Some(response),
        decoder: SseDecoder::new(MAX_SSE_FRAME_BYTES),
        buffered: Vec::new(),
        buffered_offset: 0,
        decoded_bytes: 0,
        decoded_events: 0,
        registration: after,
        service_info: after_info,
        timing,
        last_byte_at: tokio::time::Instant::now(),
    })
}

pub(crate) async fn verify_binding(
    registration_path: PathBuf,
    database_path: PathBuf,
    timeout: Duration,
) -> Result<ServiceInfo, EventStreamError> {
    super::verify_database_association(&registration_path, &database_path, timeout)
        .await
        .map_err(EventStreamError::Association)
}

#[cfg(test)]
mod tests {
    use super::*;
    use antiburn_local::analysis::opencode_v2_capture::{
        BoundaryKind, CaptureGap, CapturedEvent, MAX_CAPTURE_EVENT_BYTES,
    };

    #[test]
    fn sse_framing_does_not_reduce_the_capture_payload_limit() {
        for extra in [0, 1] {
            let mut envelope = serde_json::json!({
                "id": "evt_limit",
                "type": "session.updated",
                "durable": {"aggregateID": "ses_limit", "seq": 1},
                "properties": {"padding": ""},
            });
            let base = serde_json::to_vec(&envelope).unwrap().len();
            envelope["properties"]["padding"] =
                serde_json::json!("x".repeat(MAX_CAPTURE_EVENT_BYTES - base + extra));
            let payload = serde_json::to_string(&envelope).unwrap();
            assert_eq!(payload.len(), MAX_CAPTURE_EVENT_BYTES + extra);
            let frame = format!("id: stream-id\nevent: message\ndata: {payload}\n\n");
            let mut bytes = frame.as_bytes();
            let event = SseDecoder::new(MAX_SSE_FRAME_BYTES)
                .next_event(&mut bytes)
                .unwrap()
                .unwrap();
            let captured = CapturedEvent::new(event.data, BoundaryKind::Live);
            if extra == 0 {
                assert!(captured.is_ok());
            } else {
                assert_eq!(captured, Err(CaptureGap::CapacityExceeded));
            }
            assert!(bytes.is_empty());
        }
    }
}
