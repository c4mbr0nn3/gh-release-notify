use std::collections::HashMap;
use std::sync::Mutex;

use axum::extract::{Request, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, Duration, Utc};
use subtle::ConstantTimeEq;
use tracing::{debug, warn};

use super::AppState;

pub const PUBLIC_ROUTES: &[&str] = &["/", "/app.js", "/pico.css", "/ui.css", "/login", "/healthz"];
pub const CSRF_HEADER: &str = "x-requested-with";
pub const CSRF_VALUE: &str = "gh-release-notify";
pub const SESSION_TTL_HOURS: i64 = 12;
const LOGIN_LIMIT_PER_MINUTE: u32 = 5;

#[derive(Default)]
pub struct Sessions {
    map: Mutex<HashMap<String, DateTime<Utc>>>,
}

impl Sessions {
    pub fn create(&self) -> String {
        let mut buf = [0u8; 32];
        if getrandom::fill(&mut buf).is_err() {
            warn!("session entropy unavailable");
            return String::new();
        }
        let id: String = buf.iter().map(|b| format!("{b:02x}")).collect();
        if let Ok(mut m) = self.map.lock() {
            m.retain(|_, exp| *exp > Utc::now());
            m.insert(id.clone(), Utc::now() + Duration::hours(SESSION_TTL_HOURS));
        }
        id
    }

    pub fn validate(&self, id: &str) -> bool {
        let Ok(mut m) = self.map.lock() else {
            return false;
        };
        match m.get(id) {
            Some(exp) if *exp > Utc::now() => true,
            Some(_) => {
                m.remove(id);
                false
            }
            None => false,
        }
    }

    pub fn destroy(&self, id: &str) {
        if let Ok(mut m) = self.map.lock() {
            m.remove(id);
        }
    }
}

#[derive(Default)]
pub struct LoginLimiter {
    attempts: Mutex<HashMap<String, (u32, DateTime<Utc>)>>,
}

impl LoginLimiter {
    pub fn allow(&self, key: &str) -> bool {
        let Ok(mut m) = self.attempts.lock() else {
            return false;
        };
        let now = Utc::now();
        m.retain(|_, (_, ts)| now - *ts <= Duration::minutes(1));
        let entry = m.entry(key.to_string()).or_insert((0, now));
        entry.0 += 1;
        entry.0 <= LOGIN_LIMIT_PER_MINUTE
    }

    pub fn reset(&self, key: &str) {
        if let Ok(mut m) = self.attempts.lock() {
            m.remove(key);
        }
    }

    #[cfg(test)]
    pub(crate) fn tracked(&self) -> usize {
        self.attempts.lock().map(|m| m.len()).unwrap_or(0)
    }

    #[cfg(test)]
    pub(crate) fn expire_all_for_test(&self) {
        if let Ok(mut m) = self.attempts.lock() {
            for entry in m.values_mut() {
                entry.1 = Utc::now() - Duration::minutes(5);
            }
        }
    }
}

pub fn client_key(
    trust_proxy: bool,
    headers: &HeaderMap,
    peer: Option<std::net::SocketAddr>,
) -> String {
    if trust_proxy {
        if let Some(last) = headers
            .get("x-forwarded-for")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.split(',').next_back())
            .map(str::trim)
            .filter(|v| !v.is_empty())
        {
            return last.to_string();
        }
    }
    match peer {
        Some(addr) => addr.ip().to_string(),
        None => "unknown".to_string(),
    }
}

pub fn constant_time_eq(a: &str, b: &str) -> bool {
    a.as_bytes().ct_eq(b.as_bytes()).into()
}

pub fn session_cookie_from_headers(headers: &HeaderMap) -> Option<String> {
    let raw = headers.get(header::COOKIE)?.to_str().ok()?;
    raw.split(';')
        .filter_map(|p| p.trim().strip_prefix("grn_session="))
        .next()
        .map(|s| s.to_string())
}

pub async fn require_auth(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let path = req.uri().path().to_string();
    if PUBLIC_ROUTES.contains(&path.as_str()) {
        return next.run(req).await;
    }

    let cfg = state.config_rx.borrow().clone();
    let token = cfg.admin_token();

    let authorized = if cfg.ui.trust_proxy_auth {
        let ok = req
            .headers()
            .get("remote-user")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| !v.is_empty());
        if ok {
            debug!("proxy auth accepted for {path}");
        }
        ok
    } else if !token.is_empty() {
        let via_cookie = session_cookie_from_headers(req.headers())
            .is_some_and(|id| state.sessions.validate(&id));
        let via_bearer = req
            .headers()
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .is_some_and(|presented| constant_time_eq(presented, &token));
        via_cookie || via_bearer
    } else {
        true
    };

    if !authorized {
        warn!("unauthorized request to {path}");
        return (StatusCode::UNAUTHORIZED, "unauthorized").into_response();
    }

    let is_mutation = req.method() != axum::http::Method::GET;
    if is_mutation {
        let header_ok =
            req.headers().get(CSRF_HEADER).and_then(|v| v.to_str().ok()) == Some(CSRF_VALUE);
        if !header_ok {
            return (StatusCode::BAD_REQUEST, "missing X-Requested-With header").into_response();
        }
    }

    next.run(req).await
}

pub fn cookie_header(session: &str, secure: bool) -> String {
    let mut c = format!(
        "grn_session={session}; HttpOnly; SameSite=Strict; Path=/; Max-Age={}",
        SESSION_TTL_HOURS * 3600
    );
    if secure {
        c.push_str("; Secure");
    }
    c
}

pub fn clear_cookie_header() -> String {
    "grn_session=; HttpOnly; SameSite=Strict; Path=/; Max-Age=0".to_string()
}

pub fn is_https(headers: &HeaderMap) -> bool {
    headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        == Some("https")
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;
    use std::net::SocketAddr;

    #[test]
    fn limiter_blocks_sixth_attempt_and_resets() {
        let l = LoginLimiter::default();
        for _ in 0..5 {
            assert!(l.allow("1.2.3.4"));
        }
        assert!(!l.allow("1.2.3.4"));
        l.reset("1.2.3.4");
        assert!(l.allow("1.2.3.4"));
    }

    #[test]
    fn limiter_purges_stale_keys() {
        let l = LoginLimiter::default();
        for i in 0..200 {
            l.allow(&format!("10.0.0.{i}"));
        }
        l.expire_all_for_test();
        l.allow("10.0.0.1");
        assert_eq!(l.tracked(), 1);
    }

    #[test]
    fn client_key_uses_socket_when_not_behind_proxy() {
        let mut h = HeaderMap::new();
        h.insert("x-forwarded-for", HeaderValue::from_static("6.6.6.6"));
        let peer: SocketAddr = "10.0.0.9:5555".parse().unwrap();
        assert_eq!(client_key(false, &h, Some(peer)), "10.0.0.9");
    }

    #[test]
    fn client_key_uses_last_forwarded_entry_behind_proxy() {
        let mut h = HeaderMap::new();
        h.insert(
            "x-forwarded-for",
            HeaderValue::from_static("6.6.6.6, 203.0.113.7"),
        );
        let peer: SocketAddr = "10.0.0.9:5555".parse().unwrap();
        assert_eq!(client_key(true, &h, Some(peer)), "203.0.113.7");
    }

    #[test]
    fn client_key_falls_back_to_peer_without_forwarded_header() {
        let h = HeaderMap::new();
        let peer: SocketAddr = "10.0.0.9:5555".parse().unwrap();
        assert_eq!(client_key(true, &h, Some(peer)), "10.0.0.9");
    }
}
