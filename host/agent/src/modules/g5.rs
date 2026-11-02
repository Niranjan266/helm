//! G5 methods (`Docs/protocol/dock-deck-2.md`): `apps.usage`, `deck.icon.put`, `deck.icon.get`.
//! Dock folders and deck pages extend `dock.*` / `deck.*` in [`super::layout`].

use async_trait::async_trait;
use serde_json::{Value, json};

use super::{opt_u64, str_param};
use crate::platform::blocking;
use crate::router::{MethodSpec, Module, RequestContext, RpcError, Tier};
use crate::util;

pub struct G5Module;

const fn m(name: &'static str, min_tier: Tier) -> MethodSpec {
    MethodSpec { name, min_tier }
}

const METHODS: &[MethodSpec] = &[
    m("apps.usage", Tier::Viewer),
    m("deck.icon.put", Tier::Remote),
    m("deck.icon.get", Tier::Viewer),
];

#[async_trait]
impl Module for G5Module {
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
            "apps.usage" => {
                let limit = opt_u64(&params, "limit")?.unwrap_or(200).min(2000) as usize;
                let now = util::now_ms().max(0) as u64;
                let ranked = blocking(move || Ok(state.g5.ranked(now))).await?;
                let apps: Vec<Value> = ranked
                    .into_iter()
                    .take(limit)
                    .map(|(id, u, score)| {
                        json!({
                            "app_id": id,
                            "launches": u.launches,
                            "focus": u.focus,
                            "last_used_ms": u.last_used_ms,
                            "score": (score * 1000.0).round() / 1000.0,
                        })
                    })
                    .collect();
                Ok(json!({"apps": apps}))
            }
            "deck.icon.put" => {
                let b64 = str_param(&params, "png")?;
                if b64.len() > crate::g5::MAX_ICON_BYTES * 4 / 3 + 8 {
                    return Err(RpcError::bad_params("icon is too large"));
                }
                let bytes = util::b64_decode(b64)
                    .ok_or_else(|| RpcError::bad_params("png must be base64"))?;
                let icon = blocking(move || {
                    state
                        .g5
                        .put_icon(&bytes)
                        .map_err(RpcError::bad_params)
                })
                .await?;
                Ok(json!({"icon": icon}))
            }
            "deck.icon.get" => {
                let icon = str_param(&params, "icon")?.to_string();
                let bytes = blocking(move || {
                    state.g5.get_icon(&icon).map_err(RpcError::bad_params)
                })
                .await?;
                match bytes {
                    Some(b) => Ok(json!({"png": util::b64_encode(&b)})),
                    None => Err(RpcError::bad_params("no such icon")),
                }
            }
            _ => Err(RpcError::unknown_method(method)),
        }
    }
}

