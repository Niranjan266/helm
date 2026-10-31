//! Phase 3: `audio.sessions`, `audio.session.set`, `windows.move`, `windows.snap`,
//! `desktop.switch`.

use async_trait::async_trait;
use serde_json::{Value, json};

use super::{opt_bool, opt_f64, str_param, typed_param, u64_param};
use crate::platform::{Snap, VirtualDesktop, blocking};
use crate::router::{MethodSpec, Module, RequestContext, RpcError, Tier};

pub struct DesktopModule;

const fn m(name: &'static str, min_tier: Tier) -> MethodSpec {
    MethodSpec { name, min_tier }
}

const METHODS: &[MethodSpec] = &[
    m("audio.sessions", Tier::Viewer),
    m("audio.session.set", Tier::Remote),
    m("windows.move", Tier::Remote),
    m("windows.snap", Tier::Remote),
    m("desktop.switch", Tier::Remote),
];

#[async_trait]
impl Module for DesktopModule {
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
            "audio.sessions" => {
                let list = blocking(move || p.audio_sessions()).await?;
                let sessions: Vec<Value> = list
                    .iter()
                    .map(|s| {
                        json!({"id": s.id, "name": s.name, "pid": s.pid, "level": s.level,
                               "muted": s.muted, "is_system": s.is_system, "active": s.active})
                    })
                    .collect();
                Ok(json!({"sessions": sessions}))
            }
            "audio.session.set" => {
                let id = str_param(&params, "id")?.to_string();
                let level = opt_f64(&params, "level")?;
                let muted = opt_bool(&params, "muted")?;
                if level.is_none() && muted.is_none() {
                    return Err(RpcError::bad_params("level or muted is required"));
                }
                if let Some(l) = level
                    && !(0.0..=1.0).contains(&l)
                {
                    return Err(RpcError::bad_params("level must be 0..1"));
                }
                blocking(move || {
                    if let Some(l) = level {
                        p.set_session_volume(&id, l as f32)?;
                    }
                    if let Some(m) = muted {
                        p.set_session_muted(&id, m)?;
                    }
                    Ok(())
                })
                .await?;
                Ok(json!({}))
            }
            "windows.move" => {
                let hwnd = u64_param(&params, "hwnd")?;
                let index = usize::try_from(u64_param(&params, "monitor_index")?)
                    .map_err(|_| RpcError::bad_params("monitor_index out of range"))?;
                blocking(move || p.move_window_to_monitor(hwnd, index)).await?;
                Ok(json!({}))
            }
            "windows.snap" => {
                let hwnd = u64_param(&params, "hwnd")?;
                let pos: Snap = typed_param(&params, "pos")?;
                blocking(move || p.snap_window(hwnd, pos)).await?;
                Ok(json!({}))
            }
            "desktop.switch" => {
                let dir: VirtualDesktop = typed_param(&params, "dir")?;
                blocking(move || p.virtual_desktop(dir)).await?;
                Ok(json!({}))
            }
            _ => Err(RpcError::unknown_method(method)),
        }
    }
}
