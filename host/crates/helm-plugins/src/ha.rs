//! Home Assistant REST client (long-lived access token).
//!
//! Endpoints: `GET /api/` (ping), `GET /api/states`,
//! `GET /api/states/<entity_id>`, `POST /api/services/<domain>/<service>`.
//! Names are validated before any request so they cannot change the path.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::http::{HttpError, HttpRequest, HttpResponse, HttpTransport, Url};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HaError {
    #[error("invalid configuration: {0}")]
    InvalidConfig(String),
    #[error("invalid name `{0}`")]
    InvalidName(String),
    #[error("unauthorized (check the access token)")]
    Unauthorized,
    #[error("not found")]
    NotFound,
    #[error("bad request: {0}")]
    BadRequest(String),
    #[error("server error {status}: {body}")]
    Server { status: u16, body: String },
    #[error("unexpected HTTP status {0}")]
    UnexpectedStatus(u16),
    #[error("transport: {0}")]
    Transport(#[from] HttpError),
    #[error("cannot decode response: {0}")]
    Decode(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EntityState {
    pub entity_id: String,
    pub state: String,
    #[serde(default)]
    pub attributes: Map<String, Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_changed: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_updated: Option<String>,
}

impl EntityState {
    pub fn friendly_name(&self) -> Option<&str> {
        self.attributes.get("friendly_name").and_then(Value::as_str)
    }
}

pub struct HaClient {
    base: String,
    token: String,
    http: Arc<dyn HttpTransport>,
}

impl std::fmt::Debug for HaClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HaClient")
            .field("base", &self.base)
            .field("token", &"<redacted>")
            .finish()
    }
}

fn valid_slug(s: &str) -> bool {
    (1..=64).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

/// `domain.object_id` with lower-case letters, digits and `_`.
pub fn valid_entity_id(s: &str) -> bool {
    s.len() <= 255
        && s.split_once('.').is_some_and(|(d, o)| {
            valid_slug(d)
                && !o.is_empty()
                && o.bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
        })
}

const MAX_ERROR_BODY: usize = 512;

fn body_text(resp: &HttpResponse) -> String {
    let s = String::from_utf8_lossy(&resp.body);
    let mut s = s.trim().to_string();
    if s.len() > MAX_ERROR_BODY {
        let mut cut = MAX_ERROR_BODY;
        while !s.is_char_boundary(cut) {
            cut -= 1;
        }
        s.truncate(cut);
        s.push('…');
    }
    s
}

impl HaClient {
    /// `base_url` like `http://homeassistant.local:8123` (a path prefix is
    /// allowed for reverse proxies).
    pub fn new(base_url: &str, token: &str, http: Arc<dyn HttpTransport>) -> Result<Self, HaError> {
        let base = base_url.trim().trim_end_matches('/').to_string();
        let url = Url::parse(&base).map_err(|e| HaError::InvalidConfig(e.to_string()))?;
        if url.path_and_query.contains(['?', '#']) {
            return Err(HaError::InvalidConfig(
                "base URL must not have a query".into(),
            ));
        }
        let token = token.trim();
        if token.is_empty() || token.len() > 4096 || !token.bytes().all(|b| b.is_ascii_graphic()) {
            return Err(HaError::InvalidConfig(
                "token must be non-empty printable ASCII".into(),
            ));
        }
        Ok(Self {
            base,
            token: token.to_string(),
            http,
        })
    }

    pub fn base_url(&self) -> &str {
        &self.base
    }

    fn request(&self, mut req: HttpRequest) -> Result<HttpResponse, HaError> {
        req.headers
            .push(("Authorization".into(), format!("Bearer {}", self.token)));
        req.headers
            .push(("Accept".into(), "application/json".into()));
        let resp = self.http.send(&req)?;
        match resp.status {
            200 | 201 => Ok(resp),
            400 => Err(HaError::BadRequest(body_text(&resp))),
            401 | 403 => Err(HaError::Unauthorized),
            404 => Err(HaError::NotFound),
            s @ 500..=599 => Err(HaError::Server {
                status: s,
                body: body_text(&resp),
            }),
            s => Err(HaError::UnexpectedStatus(s)),
        }
    }

    fn json<T: serde::de::DeserializeOwned>(resp: &HttpResponse) -> Result<T, HaError> {
        serde_json::from_slice(&resp.body).map_err(|e| HaError::Decode(e.to_string()))
    }

    /// `GET /api/` – checks reachability and the token.
    pub fn ping(&self) -> Result<String, HaError> {
        let resp = self.request(HttpRequest::get(format!("{}/api/", self.base)))?;
        let v: Value = Self::json(&resp)?;
        Ok(v.get("message")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string())
    }

    pub fn states(&self) -> Result<Vec<EntityState>, HaError> {
        let resp = self.request(HttpRequest::get(format!("{}/api/states", self.base)))?;
        Self::json(&resp)
    }

    pub fn state(&self, entity_id: &str) -> Result<EntityState, HaError> {
        if !valid_entity_id(entity_id) {
            return Err(HaError::InvalidName(entity_id.into()));
        }
        let resp = self.request(HttpRequest::get(format!(
            "{}/api/states/{entity_id}",
            self.base
        )))?;
        Self::json(&resp)
    }

    /// `POST /api/services/<domain>/<service>` with `data` (an object, e.g.
    /// `{"entity_id":"light.kitchen"}`). Returns the states that changed.
    pub fn call_service(
        &self,
        domain: &str,
        service: &str,
        data: &Value,
    ) -> Result<Vec<EntityState>, HaError> {
        for n in [domain, service] {
            if !valid_slug(n) {
                return Err(HaError::InvalidName(n.into()));
            }
        }
        let body = match data {
            Value::Null => b"{}".to_vec(),
            Value::Object(_) => {
                serde_json::to_vec(data).map_err(|e| HaError::Decode(e.to_string()))?
            }
            _ => {
                return Err(HaError::InvalidConfig(
                    "service data must be a JSON object".into(),
                ));
            }
        };
        let req = HttpRequest::post(
            format!("{}/api/services/{domain}/{service}", self.base),
            body,
        )
        .header("Content-Type", "application/json");
        let resp = self.request(req)?;
        if resp.body.iter().all(u8::is_ascii_whitespace) {
            return Ok(Vec::new());
        }
        Self::json(&resp)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_validation() {
        assert!(valid_entity_id("light.kitchen_2"));
        for bad in [
            "light",
            "light.",
            ".x",
            "Light.x",
            "light.x/../y",
            "light.a b",
            "a.b?c",
        ] {
            assert!(!valid_entity_id(bad), "{bad}");
        }
        assert!(valid_slug("turn_on"));
        assert!(!valid_slug("turn/on"));
    }
}
