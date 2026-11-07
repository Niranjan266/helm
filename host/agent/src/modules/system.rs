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
                let p = ctx.state.platform.clone();
                // Wake-on-LAN data and battery are best effort: never fail system.info.
                let (macs, battery) = tokio::task::spawn_blocking(move || {
                    let macs: Vec<Value> = p
                        .network_adapters()
                        .unwrap_or_default()
                        .into_iter()
                        .map(|a| {
                            json!({
                                "mac": a.mac,
                                "adapter_name": a.adapter_name,
                                "locally_administered": a.locally_administered,
                            })
                        })
                        .collect();
                    (macs, p.battery().ok().flatten())
                })
                .await
                .map_err(|e| RpcError::internal(e.to_string()))?;
                Ok(json!({
                    "host_id": id.host_id,
                    "host_name": id.host_name,
                    "os": os,
                    "agent_version": AGENT_VERSION,
                    "uptime_s": sysinfo::uptime_s(ctx.state.started),
                    "macs": macs,
                    "battery": battery,
                    "relay": ctx.state.relay.info(),
                    // Nobody is signed in to Windows yet: only screen-less methods work.
                    "pre_login": ctx.state.pre_login,
                }))
            }
            _ => Err(RpcError::unknown_method(method)),
        }
    }
}
