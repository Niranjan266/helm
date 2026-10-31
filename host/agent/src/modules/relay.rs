//! Phase 4: `relay.get`, `relay.set`, `wol.send` (PROTOCOL §12).

use async_trait::async_trait;
use serde_json::Value;

use super::{opt_bool, opt_str, str_param};
use crate::router::{MethodSpec, Module, RequestContext, RpcError, Tier};

pub struct RelayModule;

const METHODS: &[MethodSpec] = &[
    MethodSpec {
        name: "relay.get",
        min_tier: Tier::Admin,
    },
    MethodSpec {
        name: "relay.set",
        min_tier: Tier::Admin,
    },
    MethodSpec {
        name: "wol.send",
        min_tier: Tier::Remote,
    },
];

#[async_trait]
impl Module for RelayModule {
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
            "relay.get" => Ok(state.relay.describe()),
            "relay.set" => {
                let enabled = opt_bool(&params, "enabled")?;
                let url = opt_str(&params, "url")?;
                let saved = state
                    .relay
                    .set(enabled, url)
                    .map_err(RpcError::bad_params)?;
                state.audit.record(
                    "relay_set",
                    Some(&ctx.device_id),
                    Some(&ctx.peer),
                    &format!("enabled={} url={}", saved.enabled, saved.url),
                );
                Ok(state.relay.describe())
            }
            "wol.send" => {
                let mac = str_param(&params, "mac")?;
                let mac =
                    helm_win_system::net::parse_mac(mac).map_err(crate::platform::system_err)?;
                let packet = helm_win_system::net::wol_magic_packet(&mac);
                let platform = state.platform.clone();
                crate::platform::blocking(move || platform.send_wol(&packet)).await?;
                let mac_text = helm_win_system::net::format_mac(&mac);
                state
                    .audit
                    .record("wol_send", Some(&ctx.device_id), Some(&ctx.peer), &mac_text);
                Ok(serde_json::json!({"sent": true, "mac": mac_text}))
            }
            _ => Err(RpcError::unknown_method(method)),
        }
    }
}
