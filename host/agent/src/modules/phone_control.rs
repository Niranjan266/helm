//! g9 methods the phone calls (Docs/protocol/phone-control.md). Viewer tier: this exposes the
//! *phone*, so the phone decides; being paired (an authenticated session) is all the PC requires.

use async_trait::async_trait;
use serde_json::{Value, json};

use super::{opt_bool, opt_str, opt_u64, str_param, u64_param};
use crate::phone::{PhoneError, StateChange};
use crate::router::{MethodSpec, Module, RequestContext, RpcError, Tier};

pub struct PhoneControlModule;

const METHODS: &[MethodSpec] = &[
    MethodSpec { name: "phone.caps", min_tier: Tier::Viewer },
    MethodSpec { name: "phone.accept", min_tier: Tier::Viewer },
    MethodSpec { name: "phone.decline", min_tier: Tier::Viewer },
    MethodSpec { name: "phone.state", min_tier: Tier::Viewer },
    MethodSpec { name: "phone.stop", min_tier: Tier::Viewer },
];

/// Biggest picture side the phone may announce.
const MAX_SIDE: u64 = 8192;

fn rpc(e: PhoneError) -> RpcError {
    RpcError::bad_params(e.message)
}

fn required_bool(params: &Value, name: &str) -> Result<bool, RpcError> {
    opt_bool(params, name)?
        .ok_or_else(|| RpcError::bad_params(format!("{name} (true or false) is required")))
}

fn side(params: &Value, name: &str) -> Result<u32, RpcError> {
    let v = u64_param(params, name)?;
    if !(16..=MAX_SIDE).contains(&v) {
        return Err(RpcError::bad_params(format!("{name} must be 16..={MAX_SIDE}")));
    }
    Ok(v as u32)
}

fn rotation(v: u64) -> Result<u32, RpcError> {
    match v {
        0 | 90 | 180 | 270 => Ok(v as u32),
        _ => Err(RpcError::bad_params("rotation must be 0, 90, 180 or 270")),
    }
}

fn parse_state(params: &Value) -> Result<StateChange, RpcError> {
    let optional_side = |name: &str| -> Result<Option<u32>, RpcError> {
        match opt_u64(params, name)? {
            None => Ok(None),
            Some(_) => side(params, name).map(Some),
        }
    };
    let secure = match params.get("secure") {
        None => None,
        Some(Value::Null) => Some(None),
        Some(Value::String(s)) if s == "password" => Some(Some(s.clone())),
        Some(_) => return Err(RpcError::bad_params("secure must be null or \"password\"")),
    };
    Ok(StateChange {
        width: optional_side("width")?,
        height: optional_side("height")?,
        rotation: opt_u64(params, "rotation")?.map(rotation).transpose()?,
        control: opt_bool(params, "control")?,
        accessibility: opt_bool(params, "accessibility")?,
        secure,
    })
}

#[async_trait]
impl Module for PhoneControlModule {
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
        let hub = &state.phone;
        let session_id = ctx.session.id;
        match method {
            "phone.caps" => {
                let enabled = required_bool(&params, "enabled")?;
                let accessibility = required_bool(&params, "accessibility")?;
                hub.on_caps(&ctx.session, &ctx.peer, enabled, accessibility);
                Ok(json!({}))
            }
            "phone.accept" => {
                let id = u64_param(&params, "request_id")?;
                let token = hub
                    .on_accept(
                        state,
                        &ctx.session,
                        id,
                        side(&params, "width")?,
                        side(&params, "height")?,
                        rotation(opt_u64(&params, "rotation")?.unwrap_or(0))?,
                        required_bool(&params, "control")?,
                    )
                    .map_err(rpc)?;
                Ok(json!({"token": token}))
            }
            "phone.decline" => {
                let id = u64_param(&params, "request_id")?;
                let reason = str_param(&params, "reason")?;
                hub.on_decline(state, session_id, id, reason).map_err(rpc)?;
                Ok(json!({}))
            }
            "phone.state" => {
                let id = u64_param(&params, "request_id")?;
                hub.on_state(session_id, id, parse_state(&params)?).map_err(rpc)?;
                Ok(json!({}))
            }
            "phone.stop" => {
                let id = u64_param(&params, "request_id")?;
                let reason = opt_str(&params, "reason")?.unwrap_or(crate::phone::REASON_PHONE);
                hub.on_stop(state, session_id, id, reason).map_err(rpc)?;
                Ok(json!({}))
            }
            _ => Err(RpcError::unknown_method(method)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_params() {
        let s = parse_state(&json!({"width": 720, "rotation": 90, "secure": "password"})).unwrap();
        assert_eq!(s.width, Some(720));
        assert_eq!(s.rotation, Some(90));
        assert_eq!(s.secure, Some(Some("password".into())));
        assert_eq!(parse_state(&json!({"secure": null})).unwrap().secure, Some(None));
        assert_eq!(parse_state(&json!({})).unwrap(), StateChange::default());
        for bad in [
            json!({"rotation": 45}),
            json!({"width": 2}),
            json!({"secure": "other"}),
            json!({"control": "yes"}),
        ] {
            assert!(parse_state(&bad).is_err(), "{bad}");
        }
    }
}
