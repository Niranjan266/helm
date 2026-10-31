//! `media.*`, `audio.*`, `display.brightness.*`.

use async_trait::async_trait;
use serde_json::{Value, json};

use super::{opt_bool, opt_f64, str_param, typed_param, u64_param};
use crate::platform::{MediaAction, blocking};
use crate::router::{MethodSpec, Module, RequestContext, RpcError, Tier};
use crate::util;

pub struct MediaModule;

const fn m(name: &'static str, min_tier: Tier) -> MethodSpec {
    MethodSpec { name, min_tier }
}

const METHODS: &[MethodSpec] = &[
    m("media.state", Tier::Viewer),
    m("media.artwork", Tier::Viewer),
    m("media.control", Tier::Remote),
    m("audio.get", Tier::Viewer),
    m("audio.set", Tier::Remote),
    m("display.brightness.list", Tier::Viewer),
    m("display.brightness.set", Tier::Remote),
];

#[async_trait]
impl Module for MediaModule {
    fn methods(&self) -> &'static [MethodSpec] {
        METHODS
    }

    async fn call(
        &self,
        method: &str,
        params: Value,
        ctx: &RequestContext,
    ) -> Result<Value, RpcError> {
        let p = ctx.state.platform.clone();
        match method {
            "media.state" => {
                let state = blocking(move || p.media_state()).await?;
                Ok(json!({"state": state}))
            }
            "media.artwork" => {
                let png = blocking(move || p.media_artwork_png()).await?;
                Ok(json!({"png": png.map(|b| util::b64_encode(&b))}))
            }
            "media.control" => {
                let action: MediaAction = typed_param(&params, "action")?;
                blocking(move || p.media_control(action)).await?;
                Ok(json!({}))
            }
            "audio.get" => {
                let v = blocking(move || p.volume()).await?;
                Ok(json!({"level": v.level, "muted": v.muted}))
            }
            "audio.set" => {
                let level = opt_f64(&params, "level")?;
                let muted = opt_bool(&params, "muted")?;
                if level.is_none() && muted.is_none() {
                    return Err(RpcError::bad_params("give level and/or muted"));
                }
                if let Some(l) = level
                    && !(0.0..=1.0).contains(&l)
                {
                    return Err(RpcError::bad_params("level must be between 0 and 1"));
                }
                blocking(move || {
                    if let Some(l) = level {
                        p.set_volume(l as f32)?;
                    }
                    if let Some(m) = muted {
                        p.set_muted(m)?;
                    }
                    Ok(())
                })
                .await?;
                Ok(json!({}))
            }
            "display.brightness.list" => {
                let displays = blocking(move || p.brightness_list()).await?;
                Ok(json!({"displays": displays}))
            }
            "display.brightness.set" => {
                let id = str_param(&params, "id")?.to_string();
                let level = u64_param(&params, "level")?;
                if level > 100 {
                    return Err(RpcError::bad_params("level must be 0-100"));
                }
                blocking(move || p.set_brightness(&id, level as u8)).await?;
                Ok(json!({}))
            }
            _ => Err(RpcError::unknown_method(method)),
        }
    }
}
