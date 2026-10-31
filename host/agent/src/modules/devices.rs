//! `devices.list`, `devices.revoke`.

use async_trait::async_trait;
use serde_json::{Value, json};

use crate::router::{MethodSpec, Module, RequestContext, RpcError, Tier};

pub struct DevicesModule;

const METHODS: &[MethodSpec] = &[
    MethodSpec {
        name: "devices.list",
        min_tier: Tier::Admin,
    },
    MethodSpec {
        name: "devices.revoke",
        min_tier: Tier::Admin,
    },
    MethodSpec {
        name: "devices.set_tier",
        min_tier: Tier::Admin,
    },
    MethodSpec {
        name: "audit.list",
        min_tier: Tier::Admin,
    },
];

/// Most lines `audit.list` returns.
const MAX_AUDIT_LINES: u64 = 1000;

#[async_trait]
impl Module for DevicesModule {
    fn methods(&self) -> &'static [MethodSpec] {
        METHODS
    }

    async fn call(
        &self,
        method: &str,
        params: Value,
        ctx: &RequestContext,
    ) -> Result<Value, RpcError> {
        let state = ctx.state.clone();
        match method {
            "devices.list" => {
                let devices = state
                    .devices
                    .list()
                    .map_err(|e| RpcError::internal(e.to_string()))?;
                let list: Vec<Value> = devices
                    .iter()
                    .map(|d| {
                        json!({
                            "device_id": d.device_id,
                            "device_name": d.device_name,
                            "tier": d.tier.as_str(),
                            "paired_at_ms": d.paired_at_ms,
                            "last_seen_ms": d.last_seen_ms,
                            "this": d.device_id == ctx.device_id,
                        })
                    })
                    .collect();
                Ok(json!({"devices": list}))
            }
            "devices.revoke" => {
                let target = params
                    .get("device_id")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| RpcError::bad_params("device_id (string) is required"))?
                    .to_string();
                let removed = state
                    .devices
                    .revoke(&target)
                    .map_err(|e| RpcError::internal(e.to_string()))?;
                if !removed {
                    // Already gone: nothing to do (idempotent).
                    return Ok(json!({"revoked": false}));
                }
                state.audit.record(
                    "revoke",
                    Some(&target),
                    Some(&ctx.peer),
                    &format!("by={}", ctx.device_id),
                );
                tracing::info!("device {target} revoked by {}", ctx.device_id);
                // Disconnect the revoked device's other sessions now. The caller's own session
                // (self-revoke) is closed by the server after this response is sent.
                state.kick_device(&target, Some(ctx.session_id));
                Ok(json!({"revoked": true}))
            }
            "devices.set_tier" => {
                let target = super::str_param(&params, "device_id")?.to_string();
                let tier: Tier = super::typed_param(&params, "tier")?;
                if target == ctx.device_id {
                    return Err(RpcError::bad_params(
                        "a device cannot change its own tier (use another admin device)",
                    ));
                }
                let found = state
                    .devices
                    .set_tier(&target, tier)
                    .map_err(|e| RpcError::internal(e.to_string()))?;
                if !found {
                    return Err(RpcError::bad_params("no such device"));
                }
                state.audit.record(
                    "set_tier",
                    Some(&target),
                    Some(&ctx.peer),
                    &format!("tier={} by={}", tier.as_str(), ctx.device_id),
                );
                // Open sessions keep the tier they authenticated with; make them reconnect.
                state.kick_device(&target, None);
                Ok(json!({}))
            }
            "audit.list" => {
                let limit = super::opt_u64(&params, "limit")?
                    .unwrap_or(200)
                    .min(MAX_AUDIT_LINES) as usize;
                let st = state.clone();
                let lines = crate::platform::blocking(move || {
                    st.audit
                        .tail(limit)
                        .map_err(|e| RpcError::internal(e.to_string()))
                })
                .await?;
                Ok(json!({"lines": lines}))
            }
            _ => Err(RpcError::unknown_method(method)),
        }
    }
}
