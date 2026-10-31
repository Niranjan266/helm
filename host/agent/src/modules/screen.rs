//! `screen.monitors`, `screen.capture`.

use async_trait::async_trait;
use serde_json::{Value, json};

use super::opt_str;
use crate::platform::blocking;
use crate::router::{MethodSpec, Module, RequestContext, RpcError, Tier};
use crate::util;

pub struct ScreenModule;

const METHODS: &[MethodSpec] = &[
    MethodSpec {
        name: "screen.monitors",
        min_tier: Tier::Viewer,
    },
    MethodSpec {
        name: "screen.capture",
        min_tier: Tier::Remote,
    },
];

#[async_trait]
impl Module for ScreenModule {
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
            "screen.monitors" => {
                let monitors = blocking(move || p.monitors()).await?;
                Ok(json!({"monitors": monitors}))
            }
            "screen.capture" => {
                let id = opt_str(&params, "monitor_id")?.map(str::to_string);
                let png = blocking(move || p.screenshot_png(id.as_deref())).await?;
                Ok(json!({"png": util::b64_encode(&png)}))
            }
            _ => Err(RpcError::unknown_method(method)),
        }
    }
}
