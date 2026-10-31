//! `clipboard.get`, `clipboard.set`, `clipboard.watch`.

use async_trait::async_trait;
use serde_json::{Map, Value, json};

use super::{opt_bool, opt_str};
use crate::platform::blocking;
use crate::router::{MethodSpec, Module, RequestContext, RpcError, Tier};
use crate::util;

pub struct ClipboardModule;

const fn m(name: &'static str, min_tier: Tier) -> MethodSpec {
    MethodSpec { name, min_tier }
}

const METHODS: &[MethodSpec] = &[
    m("clipboard.get", Tier::Remote),
    m("clipboard.set", Tier::Remote),
    m("clipboard.watch", Tier::Remote),
];

#[async_trait]
impl Module for ClipboardModule {
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
            "clipboard.get" => {
                let (text, png) =
                    blocking(move || Ok((p.clipboard_text()?, p.clipboard_png()?))).await?;
                let mut out = Map::new();
                if let Some(t) = text {
                    out.insert("text".into(), Value::String(t));
                }
                if let Some(png) = png {
                    out.insert("png".into(), Value::String(util::b64_encode(&png)));
                }
                Ok(Value::Object(out))
            }
            "clipboard.set" => {
                let text = opt_str(&params, "text")?.map(str::to_string);
                let png = opt_str(&params, "png")?;
                match (text, png) {
                    (Some(text), None) => blocking(move || p.set_clipboard_text(&text)).await?,
                    (None, Some(b64)) => {
                        let bytes = util::b64_decode(b64)
                            .ok_or_else(|| RpcError::bad_params("png must be base64"))?;
                        blocking(move || p.set_clipboard_png(&bytes)).await?
                    }
                    _ => return Err(RpcError::bad_params("give exactly one of text or png")),
                }
                Ok(json!({}))
            }
            "clipboard.watch" => {
                let on = opt_bool(&params, "on")?
                    .ok_or_else(|| RpcError::bad_params("on (bool) is required"))?;
                ctx.session.set_clipboard_watch(on);
                Ok(json!({}))
            }
            _ => Err(RpcError::unknown_method(method)),
        }
    }
}
