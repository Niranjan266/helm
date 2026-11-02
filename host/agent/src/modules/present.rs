//! `present.*` (`Docs/protocol/presentation.md`, feature `g2`): presentation state, slide
//! keys and laser settings. Laser movement is the `lp` input frame (`session.rs`).

use async_trait::async_trait;
use serde_json::{Value, json};

use super::{opt_bool, opt_str, str_param};
use crate::platform::{InputEvent, Rect, blocking};
use crate::present::{GoAction, classify, keys_for};
use crate::router::{MethodSpec, Module, RequestContext, RpcError, Tier};

pub struct PresentModule;

const METHODS: &[MethodSpec] = &[
    MethodSpec {
        name: "present.state",
        min_tier: Tier::Viewer,
    },
    MethodSpec {
        name: "present.go",
        min_tier: Tier::Remote,
    },
    MethodSpec {
        name: "present.laser",
        min_tier: Tier::Remote,
    },
];

#[async_trait]
impl Module for PresentModule {
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
            "present.state" => {
                let hub = p.presenter();
                blocking(move || {
                    let fg = hub.backend().foreground()?;
                    let detected = fg.as_ref().and_then(classify);
                    let (slide, slide_status) = match hub.backend().powerpoint() {
                        Ok(Some(s)) => (json!(s), "ok"),
                        Ok(None) => (Value::Null, "no_presentation"),
                        Err(e) if e.code == "unsupported" => (Value::Null, "unsupported"),
                        Err(e) => {
                            tracing::debug!("PowerPoint: {}", e.message);
                            (Value::Null, "busy")
                        }
                    };
                    Ok(json!({
                        "app": detected.as_ref().map(|d| d.app),
                        "title": fg.as_ref().map(|f| f.title.clone()),
                        "exe": fg.as_ref().map(|f| f.exe_name.clone()),
                        "slide": slide,
                        "slide_status": slide_status,
                        "laser": {"visible": hub.laser_visible(), "monitor_id": hub.laser_monitor()},
                    }))
                })
                .await
            }
            "present.go" => {
                let action = str_param(&params, "action")?;
                let go = GoAction::parse(action).ok_or_else(|| {
                    RpcError::bad_params(
                        "action must be next, prev, first, last, black, start or end",
                    )
                })?;
                let hub = p.presenter();
                blocking(move || {
                    let fg = hub.backend().foreground()?;
                    let detected = fg.as_ref().and_then(classify);
                    let combo = keys_for(detected.as_ref(), go).ok_or_else(|| {
                        RpcError::unsupported(match &detected {
                            Some(_) => "this presentation app has no key for that",
                            None => "no presentation app is in front for that action",
                        })
                    })?;
                    let keys = helm_win_input::parse_shortcut(combo)
                        .ok_or_else(|| RpcError::internal(format!("bad key {combo}")))?;
                    let injector = p.new_injector()?;
                    let r = injector.inject(&InputEvent::Shortcut { keys });
                    let _ = injector.release_all();
                    r.map_err(|e| match e {
                        helm_win_input::Error::BlockedByElevation => RpcError::internal(
                            "blocked_by_elevation: the foreground window is elevated",
                        ),
                        other => RpcError::internal(other.to_string()),
                    })?;
                    Ok(json!({"app": detected.map(|d| d.app), "keys": combo}))
                })
                .await
            }
            "present.laser" => {
                let monitor_id = opt_str(&params, "monitor_id")?.map(str::to_string);
                let trail = opt_bool(&params, "trail")?.unwrap_or(true);
                let session = ctx.session_id;
                blocking(move || {
                    let monitor = match &monitor_id {
                        None => None,
                        Some(id) => Some(
                            p.monitors()?
                                .into_iter()
                                .find(|m| &m.id == id)
                                .ok_or_else(|| RpcError::bad_params(format!("unknown monitor {id}")))?,
                        ),
                    };
                    let rect = monitor.as_ref().map(Rect::from);
                    p.presenter().configure(session, rect, monitor_id, trail);
                    Ok(json!({"monitor": monitor, "trail": trail}))
                })
                .await
            }
            _ => Err(RpcError::unknown_method(method)),
        }
    }
}
