//! g8 methods (Docs/protocol/pointer.md): `pointer.settings`, `pointer.set`.

use async_trait::async_trait;
use serde_json::{Value, json};

use super::{internal, str_param};
use crate::pointer::PointerMode;
use crate::router::{MethodSpec, Module, RequestContext, RpcError, Tier};

pub struct PointerModule;

const METHODS: &[MethodSpec] = &[
    MethodSpec {
        name: "pointer.settings",
        min_tier: Tier::Viewer,
    },
    MethodSpec {
        name: "pointer.set",
        min_tier: Tier::Remote,
    },
];

#[async_trait]
impl Module for PointerModule {
    fn methods(&self) -> &'static [MethodSpec] {
        METHODS
    }

    async fn call(
        &self,
        method: &str,
        params: Value,
        ctx: &RequestContext,
    ) -> Result<Value, RpcError> {
        let state = &ctx.state;
        match method {
            "pointer.settings" => Ok(json!({"mode": state.pointer.mode().as_str()})),
            "pointer.set" => {
                let raw = str_param(&params, "mode")?;
                let mode = PointerMode::parse(raw).ok_or_else(|| {
                    RpcError::bad_params("mode must be \"auto\", \"absolute\" or \"relative\"")
                })?;
                state.pointer.set_mode(mode).map_err(internal)?;
                state.audit.record(
                    "pointer_mode",
                    Some(&ctx.device_id),
                    Some(&ctx.peer),
                    &format!("mode={}", mode.as_str()),
                );
                Ok(json!({"mode": mode.as_str()}))
            }
            _ => Err(RpcError::unknown_method(method)),
        }
    }
}
