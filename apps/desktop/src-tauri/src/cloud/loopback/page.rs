//! Render fixed browser feedback without using any values from the request.

use std::sync::LazyLock;

use base64::{Engine, engine::general_purpose::STANDARD};
use sha2::{Digest, Sha256};

use crate::cloud::auth::Callback;

const STYLE: &str = include_str!("../../../../src/onboarding/cloud-loopback.css");
const BRAND_MARK: &str = include_str!("../../../../src/assets/antiburn-mark.svg");
// HTML normalizes CRLF before browsers check the style hash.
static STYLE_SOURCE: LazyLock<String> =
    LazyLock::new(|| STANDARD.encode(Sha256::digest(STYLE.replace("\r\n", "\n"))));

/// Report the browser result only. The app still has to exchange a valid code.
pub(super) fn response(callback: Option<&Callback>) -> String {
    let (status, title, detail, next) = match callback {
        Some(Callback::Code(_)) => (
            "200 OK",
            "Browser step complete",
            "Your approval reached Antiburn.",
            "Return to Antiburn to finish signing in.",
        ),
        Some(Callback::Cancelled) => (
            "200 OK",
            "Device access denied",
            "This sign-in request was not approved.",
            "Return to Antiburn if you want to try again.",
        ),
        None => (
            "400 Bad Request",
            "Could not finish sign-in",
            "This browser return link is not valid.",
            "Return to Antiburn and start sign-in again.",
        ),
    };
    let body = format!(
        r#"<!doctype html>
        <html lang="en"><head><meta charset="utf-8">
        <meta name="viewport" content="width=device-width, initial-scale=1">
        <title>{title} | Antiburn</title><style>{STYLE}</style></head>
        <body><main><header><span aria-hidden="true">{BRAND_MARK}</span>Antiburn Cloud</header>
        <h1>{title}</h1><p>{detail}</p><p class="next">{next}</p>
        </main></body></html>"#
    );
    format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\n\
         Cache-Control: no-store\r\nReferrer-Policy: no-referrer\r\n\
         Content-Security-Policy: default-src 'none'; base-uri 'none'; frame-ancestors 'none'; form-action 'none'; style-src 'sha256-{}'\r\n\
         X-Content-Type-Options: nosniff\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}",
        *STYLE_SOURCE,
        body.len()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn feedback_reports_only_the_browser_step_and_loads_no_assets() {
        let code = Callback::Code("synthetic-callback-code".into());
        for (input, status, heading) in [
            (Some(&code), "200 OK", "Browser step complete"),
            (Some(&Callback::Cancelled), "200 OK", "Device access denied"),
            (None, "400 Bad Request", "Could not finish sign-in"),
        ] {
            let reply = response(input);
            let (headers, body) = reply.split_once("\r\n\r\n").unwrap();
            assert!(headers.starts_with(&format!("HTTP/1.1 {status}\r\n")));
            assert!(headers.contains("\r\nContent-Type: text/html; charset=utf-8\r\n"));
            assert!(headers.contains("\r\nCache-Control: no-store\r\n"));
            assert!(headers.contains("\r\nReferrer-Policy: no-referrer\r\n"));
            assert!(headers.contains(&format!(
                "\r\nContent-Security-Policy: default-src 'none'; base-uri 'none'; frame-ancestors 'none'; form-action 'none'; style-src 'sha256-{}'\r\n",
                *STYLE_SOURCE
            )));
            assert!(headers.ends_with(&format!("\r\nContent-Length: {}", body.len())));
            assert!(body.contains(&format!("<h1>{heading}</h1>")));
            assert!(body.contains("Return to Antiburn"));
            assert!(body.contains(&format!("<style>{STYLE}</style>")));
            assert!(!body.contains("synthetic-callback-code"));
            for tag in ["<script", "<link", "<img", "<form", "<a "] {
                assert!(!body.contains(tag), "{tag}");
            }
        }
    }
}
