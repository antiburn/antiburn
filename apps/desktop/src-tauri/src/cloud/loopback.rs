use super::auth::{Attempt, Callback, Config, Failure, validate_callback};
use std::{
    io::{Read, Write},
    net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, TcpStream},
    time::{Duration, Instant},
};

pub struct Listener {
    listener: TcpListener,
    callback: String,
}

impl Listener {
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

    pub fn callback(&self) -> &str {
        &self.callback
    }

    pub fn wait(
        self,
        config: &Config,
        attempt: &Attempt,
        current: impl Fn() -> bool,
    ) -> Option<Callback> {
        let deadline = Instant::now() + Duration::from_secs(600);
        while current() && Instant::now() < deadline {
            match self.listener.accept() {
                Ok((mut stream, peer)) if peer.ip().is_loopback() => {
                    let result = request(&mut stream).and_then(|(target, host)| {
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
                    let _ = write!(
                        stream,
                        "HTTP/1.1 {status}\r\nContent-Type: text/plain; charset=utf-8\r\nCache-Control: no-store\r\nReferrer-Policy: no-referrer\r\nContent-Security-Policy: default-src 'none'\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}",
                        body.len()
                    );
                    if result.is_some() {
                        return result;
                    }
                }
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(25));
                }
                Err(_) => return None,
            }
        }
        None
    }
}

fn request(stream: &mut TcpStream) -> Option<(String, String)> {
    stream
        .set_read_timeout(Some(Duration::from_millis(250)))
        .ok()?;
    stream
        .set_write_timeout(Some(Duration::from_millis(250)))
        .ok()?;
    let deadline = Instant::now() + Duration::from_secs(1);
    let mut bytes = Vec::new();
    let mut buffer = [0u8; 1024];
    while bytes.len() <= 12_288 && Instant::now() < deadline {
        let count = stream.read(&mut buffer).ok()?;
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
