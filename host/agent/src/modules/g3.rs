//! G3 (`Docs/protocol/streaming-2.md`): `windows.thumb{hwnd, size?}` for the phone's window
//! picker. Window streaming itself is `stream.start{hwnd}` (§10) and the gaming preset is
//! `stream.start{preset:"gaming"}` (see `stream.rs`).

use async_trait::async_trait;
use serde_json::{Value, json};

use super::{opt_u64, u64_param};
use crate::platform::blocking;
use crate::router::{MethodSpec, Module, RequestContext, RpcError, Tier};
use crate::util;

pub struct G3Module;

const METHODS: &[MethodSpec] = &[MethodSpec {
    name: "windows.thumb",
    min_tier: Tier::Remote,
}];

pub const THUMB_DEFAULT: u64 = 256;
pub const THUMB_MIN: u64 = 32;
pub const THUMB_MAX: u64 = 512;

/// `(hwnd, size)` of a `windows.thumb` request.
pub fn parse_thumb(params: &Value) -> Result<(u64, u32), RpcError> {
    let hwnd = u64_param(params, "hwnd")?;
    if hwnd == 0 {
        return Err(RpcError::bad_params("hwnd must not be 0"));
    }
    let size = opt_u64(params, "size")?.unwrap_or(THUMB_DEFAULT);
    if !(THUMB_MIN..=THUMB_MAX).contains(&size) {
        return Err(RpcError::bad_params(format!("size must be {THUMB_MIN}..={THUMB_MAX}")));
    }
    Ok((hwnd, size as u32))
}

#[async_trait]
impl Module for G3Module {
    fn methods(&self) -> &'static [MethodSpec] {
        METHODS
    }

    async fn call(
        &self,
        method: &str,
        params: Value,
        ctx: &RequestContext,
    ) -> Result<Value, RpcError> {
        match method {
            "windows.thumb" => {
                let (hwnd, size) = parse_thumb(&params)?;
                let p = ctx.state.platform.clone();
                let png = blocking(move || p.window_thumbnail_png(hwnd, size)).await?;
                Ok(json!({"png": png.map(|b| util::b64_encode(&b))}))
            }
            _ => Err(RpcError::unknown_method(method)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thumb_params() {
        assert_eq!(parse_thumb(&json!({"hwnd": 42})).unwrap(), (42, 256));
        assert_eq!(parse_thumb(&json!({"hwnd": 42, "size": 32})).unwrap(), (42, 32));
        for bad in [
            json!({}),
            json!({"hwnd": 0}),
            json!({"hwnd": "42"}),
            json!({"hwnd": 1, "size": 31}),
            json!({"hwnd": 1, "size": 513}),
        ] {
            assert_eq!(parse_thumb(&bad).unwrap_err().code, "bad_params", "{bad}");
        }
    }
}
