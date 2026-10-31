//! `system.ping`, `system.info`.

use async_trait::async_trait;
use serde_json::{Value, json};

use crate::proto::AGENT_VERSION;
use crate::router::{MethodSpec, Module, RequestContext, RpcError, Tier};
use crate::{sysinfo, util};

pub struct SystemModule;

const METHODS: &[MethodSpec] = &[
    MethodSpec {
        name: "system.ping",
        min_tier: Tier::Viewer,
    },
    MethodSpec {
        name: "system.info",
        min_tier: Tier::Viewer,
    },
];

#[async_trait]
impl Module for SystemModule {
    fn methods(&self) -> &'static [MethodSpec] {
        METHODS
    }

    async fn call(
        &self,
        method: &str,
        _params: Value,
        ctx: &RequestContext,
    ) -> Result<Value, RpcError> {
        match method {
            "system.ping" => Ok(json!({"pong": true, "time_ms": util::now_ms()})),
            "system.info" => {
                let id = &ctx.state.identity;
                let os = tokio::task::spawn_blocking(sysinfo::os_string)
                    .await
                    .map_err(|e| RpcError::internal(e.to_string()))?;
                Ok(json!({
                    "host_id": id.host_id,
                    "host_name": id.host_name,
                    "os": os,
                    "agent_version": AGENT_VERSION,
                    "uptime_s": sysinfo::uptime_s(ctx.state.started),
                }))
            }
            _ => Err(RpcError::unknown_method(method)),
        }
    }
}
