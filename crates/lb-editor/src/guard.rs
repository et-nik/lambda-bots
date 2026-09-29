//! Who may use the editor. It listens on loopback only, yet any page open in the browser could call it, so:
//! - the printed link's token becomes a cookie, and every request needs it (one cookie a port: browsers keep cookies
//!   by host alone, and two editors, a local one and a tunnelled one, would take each other's);
//! - the `Host` header must be a loopback name, so a page that rebinds its domain to 127.0.0.1 gets nothing;
//! - requests that change anything carry `X-LB: 1`, which other origins cannot send without asking first.
//!
//! The token gives what rcon gives: the editor saves map markup and (later) sends commands to the server.

use std::sync::Arc;

use axum::extract::{Request, State};
use axum::http::{HeaderMap, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use subtle::ConstantTimeEq;

use crate::AppState;

pub const COOKIE: &str = "lb_editor";

/// The cookie of the editor the page reaches through `Host`: `lb_editor_<port>`, or `lb_editor` without a port.
pub fn cookie_name(headers: &HeaderMap) -> String {
    let port = headers
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.rsplit_once(':'))
        .map(|(_, p)| p)
        .filter(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()));
    match port {
        Some(p) => format!("{COOKIE}_{p}"),
        None => COOKIE.to_string(),
    }
}

/// A random token: 32 bytes in hex.
pub fn new_token() -> String {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).expect("the OS gives random bytes");
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn same(a: &str, b: &str) -> bool {
    a.as_bytes().ct_eq(b.as_bytes()).into()
}

/// `127.0.0.1`, `localhost` or `[::1]`, with any port: an SSH tunnel may bring the page in on another one.
fn loopback_host(headers: &HeaderMap) -> bool {
    let Some(host) = headers.get(header::HOST).and_then(|h| h.to_str().ok()) else {
        return false;
    };
    let name = if let Some(rest) = host.strip_prefix('[') {
        rest.split(']').next().map(|h| format!("[{h}]"))
    } else {
        host.split(':').next().map(str::to_string)
    };
    matches!(name.as_deref(), Some("127.0.0.1" | "localhost" | "[::1]"))
}

fn cookie<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .find_map(|kv| kv.trim().strip_prefix(name)?.strip_prefix('='))
}

fn query_token(req: &Request) -> Option<&str> {
    req.uri().query()?.split('&').find_map(|kv| kv.strip_prefix("token="))
}

fn refuse(why: &'static str) -> Response {
    (
        StatusCode::FORBIDDEN,
        [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
        format!("{why}\nOpen the link lb-editor printed when it started.\n"),
    )
        .into_response()
}

pub async fn guard(State(state): State<Arc<AppState>>, req: Request, next: Next) -> Response {
    if !loopback_host(req.headers()) {
        return refuse("The editor answers only on 127.0.0.1 or localhost.");
    }
    if let Some(token) = query_token(&req) {
        if req.uri().path() != "/" || !same(token, &state.token) {
            return refuse("Wrong token.");
        }
        return (
            StatusCode::SEE_OTHER,
            [
                (header::LOCATION, "/".to_string()),
                (
                    header::SET_COOKIE,
                    format!(
                        "{}={}; Path=/; HttpOnly; SameSite=Strict",
                        cookie_name(req.headers()),
                        state.token
                    ),
                ),
            ],
        )
            .into_response();
    }
    if !cookie(req.headers(), &cookie_name(req.headers())).is_some_and(|c| same(c, &state.token)) {
        return refuse("No access.");
    }
    let reads = matches!(*req.method(), Method::GET | Method::HEAD);
    if !reads && req.headers().get("x-lb").is_none_or(|v| v != "1") {
        return refuse("Changes need the X-LB header.");
    }
    next.run(req).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn headers(pairs: &[(header::HeaderName, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.append(k.clone(), HeaderValue::from_str(v).unwrap());
        }
        h
    }

    #[test]
    fn only_loopback_names_pass() {
        for ok in ["127.0.0.1:8090", "localhost:9000", "[::1]:8090", "127.0.0.1"] {
            assert!(loopback_host(&headers(&[(header::HOST, ok)])), "{ok}");
        }
        for bad in [
            "evil.example:8090",
            "127.0.0.1.nip.io:8090",
            "[::2]:8090",
            "localhost.evil:80",
        ] {
            assert!(!loopback_host(&headers(&[(header::HOST, bad)])), "{bad}");
        }
        assert!(!loopback_host(&HeaderMap::new()));
    }

    #[test]
    fn the_cookie_is_found_among_others() {
        let h = headers(&[(header::COOKIE, "a=1; lb_editor=abc"), (header::COOKIE, "b=2")]);
        assert_eq!(cookie(&h, COOKIE), Some("abc"));
        assert_eq!(cookie(&headers(&[(header::COOKIE, "lb_editorx=1")]), COOKIE), None);
    }

    #[test]
    fn each_port_has_its_cookie() {
        assert_eq!(
            cookie_name(&headers(&[(header::HOST, "127.0.0.1:8090")])),
            "lb_editor_8090"
        );
        assert_eq!(cookie_name(&headers(&[(header::HOST, "[::1]:9000")])), "lb_editor_9000");
        assert_eq!(cookie_name(&headers(&[(header::HOST, "localhost")])), "lb_editor");
        assert_eq!(cookie_name(&headers(&[(header::HOST, "[::1]")])), "lb_editor");
    }
}
