//! Request routing: modules declare methods with a minimum tier; the router checks the tier
//! and the params shape, then calls the module.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::server::AppState;

/// Permission tier. Ordering matters: viewer < remote < admin.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Tier {
    Viewer,
    Remote,
    Admin,
}

impl Tier {
    pub fn as_str(self) -> &'static str {
        match self {
            Tier::Viewer => "viewer",
            Tier::Remote => "remote",
            Tier::Admin => "admin",
        }
    }
}

/// An error returned in a `res` with `ok:false`.
#[derive(Debug, Clone, PartialEq)]
pub struct RpcError {
    pub code: &'static str,
    pub message: String,
}

impl RpcError {
    pub fn new(code: &'static str, message: impl Into<String>) -> RpcError {
        RpcError {
            code,
            message: message.into(),
        }
    }
    pub fn unknown_method(method: &str) -> RpcError {
        Self::new("unknown_method", format!("no method named \"{method}\""))
    }
    pub fn bad_params(message: impl Into<String>) -> RpcError {
        Self::new("bad_params", message)
    }
    pub fn forbidden(message: impl Into<String>) -> RpcError {
        Self::new("forbidden", message)
    }
    pub fn internal(message: impl Into<String>) -> RpcError {
        Self::new("internal", message)
    }
    /// The feature exists in the protocol but this PC / build cannot do it.
    pub fn unsupported(message: impl Into<String>) -> RpcError {
        Self::new("unsupported", message)
    }
}

/// Who is calling, available to every method.
#[derive(Clone)]
pub struct RequestContext {
    pub device_id: String,
    pub tier: Tier,
    pub session_id: u64,
    /// Id of the request being handled.
    pub req_id: i64,
    /// Remote address of the connection, for the audit log.
    pub peer: String,
    pub state: Arc<AppState>,
    /// The calling session (send events to it, run work after the response).
    pub session: Arc<crate::session::Session>,
}

/// One method a module offers.
#[derive(Clone, Copy, Debug)]
pub struct MethodSpec {
    pub name: &'static str,
    pub min_tier: Tier,
}

#[async_trait]
pub trait Module: Send + Sync {
    /// Methods this module handles (full names like `system.ping`).
    fn methods(&self) -> &'static [MethodSpec];

    /// Handle a call. `params` is always a JSON object.
    async fn call(
        &self,
        method: &str,
        params: Value,
        ctx: &RequestContext,
    ) -> Result<Value, RpcError>;
}

pub struct Router {
    modules: Vec<Arc<dyn Module>>,
    index: HashMap<&'static str, (Tier, usize)>,
}

impl Router {
    pub fn new(modules: Vec<Arc<dyn Module>>) -> Router {
        let mut index = HashMap::new();
        for (i, m) in modules.iter().enumerate() {
            for spec in m.methods() {
                let previous = index.insert(spec.name, (spec.min_tier, i));
                assert!(previous.is_none(), "method {} registered twice", spec.name);
            }
        }
        Router { modules, index }
    }

    pub fn method_names(&self) -> Vec<&'static str> {
        let mut names: Vec<_> = self.index.keys().copied().collect();
        names.sort_unstable();
        names
    }

    pub async fn dispatch(
        &self,
        method: &str,
        params: Value,
        ctx: &RequestContext,
    ) -> Result<Value, RpcError> {
        let Some(&(min_tier, i)) = self.index.get(method) else {
            return Err(RpcError::unknown_method(method));
        };
        if ctx.tier < min_tier {
            return Err(RpcError::forbidden(format!(
                "{method} needs tier {}, this device is {}",
                min_tier.as_str(),
                ctx.tier.as_str()
            )));
        }
        if !params.is_object() {
            return Err(RpcError::bad_params("params must be a JSON object"));
        }
        self.modules[i].call(method, params, ctx).await
    }
}
