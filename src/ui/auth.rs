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

pub const PUBLIC_ROUTES: &[&str] = &["/", "/app.js", "/pico.css", "/login", "/healthz"];
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

    #[allow(dead_code)]
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
    pub fn allow(&self, ip: &str) -> bool {
        let Ok(mut m) = self.attempts.lock() else {
            return false;
        };
        let now = Utc::now();
        let entry = m.entry(ip.to_string()).or_insert((0, now));
        if now - entry.1 > Duration::minutes(1) {
            *entry = (1, now);
            return true;
        }
        entry.0 += 1;
        entry.0 <= LOGIN_LIMIT_PER_MINUTE
    }

    pub fn reset(&self, ip: &str) {
        if let Ok(mut m) = self.attempts.lock() {
            m.remove(ip);
        }
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

#[allow(dead_code)]
pub fn clear_cookie_header() -> String {
    "grn_session=; HttpOnly; SameSite=Strict; Path=/; Max-Age=0".to_string()
}

pub fn is_https(headers: &HeaderMap) -> bool {
    headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        == Some("https")
}
