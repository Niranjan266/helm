//! `power.action`, `power.cancel`, `power.status`.
//!
//! Real actions only happen through `AppState::power_backend` / `AppState::power`, which
//! are dry-run recorders when `Config::power_dry_run` is set (tests).

use std::time::Duration;

use async_trait::async_trait;
use helm_win_system::power::{PowerAction, ScheduledPower};
use serde_json::{Value, json};

use super::{opt_u64, typed_param};
use crate::router::{MethodSpec, Module, RequestContext, RpcError, Tier};

pub struct PowerModule;

const fn m(name: &'static str, min_tier: Tier) -> MethodSpec {
    MethodSpec { name, min_tier }
}

const METHODS: &[MethodSpec] = &[
    m("power.action", Tier::Admin),
    m("power.cancel", Tier::Admin),
    m("power.status", Tier::Viewer),
];

/// Immediate actions run this long after the response was queued, so it reaches the phone.
const IMMEDIATE_GRACE: Duration = Duration::from_millis(300);

pub fn scheduled_json(s: Option<ScheduledPower>) -> Value {
    match s {
        Some(s) => json!({"action": s.action, "due_unix_ms": s.due_unix_ms}),
        None => Value::Null,
    }
}

#[async_trait]
impl Module for PowerModule {
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
            "power.action" => {
                let action: PowerAction = typed_param(&params, "action")?;
                let delay_s = opt_u64(&params, "delay_s")?.unwrap_or(0);
                state.audit.record(
                    "power",
                    Some(&ctx.device_id),
                    Some(&ctx.peer),
                    &format!("action={action:?} delay_s={delay_s}"),
                );
                if delay_s == 0 {
                    // Replace any pending timer: the user asked for something now.
                    if state.power.cancel().is_some() {
                        state.broadcast("power.scheduled", json!({"scheduled": null}));
                    }
                    let backend = state.power_backend.clone();
                    let st = state.clone();
                    ctx.session.after_response(
                        ctx.req_id,
                        Box::pin(async move {
                            tokio::time::sleep(IMMEDIATE_GRACE).await;
                            let r =
                                tokio::task::spawn_blocking(move || backend.execute(action)).await;
                            if let Ok(Err(e)) = r {
                                tracing::warn!("power action {action:?} failed: {e}");
                                st.audit.record(
                                    "power_fail",
                                    None,
                                    None,
                                    &format!("{action:?}: {e}"),
                                );
                            }
                        }),
                    );
                    return Ok(json!({"scheduled": null}));
                }
                let scheduled = state
                    .power
                    .schedule(action, Duration::from_secs(delay_s))
                    .map_err(crate::platform::system_err)?;
                let s = scheduled_json(Some(scheduled));
                state.broadcast("power.scheduled", json!({"scheduled": s}));
                Ok(json!({"scheduled": s}))
            }
            "power.cancel" => {
                if let Some(c) = state.power.cancel() {
                    state.audit.record(
                        "power_cancel",
                        Some(&ctx.device_id),
                        Some(&ctx.peer),
                        &format!("action={:?}", c.action),
                    );
                    state.broadcast("power.scheduled", json!({"scheduled": null}));
                }
                Ok(json!({"scheduled": null}))
            }
            "power.status" => Ok(json!({"scheduled": scheduled_json(state.power.status())})),
            _ => Err(RpcError::unknown_method(method)),
        }
    }
}
