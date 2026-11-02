//! `apps.*`, `open`, `windows.*`.

use async_trait::async_trait;
use serde_json::{Value, json};

use super::{check_open_target, opt_u64, str_param, u64_param};
use crate::platform::{AppKind, WindowCommand, blocking};
use crate::router::{MethodSpec, Module, RequestContext, RpcError, Tier};
use crate::util;

pub struct AppsModule;

const fn m(name: &'static str, min_tier: Tier) -> MethodSpec {
    MethodSpec { name, min_tier }
}

const METHODS: &[MethodSpec] = &[
    m("apps.list", Tier::Viewer),
    m("apps.icon", Tier::Viewer),
    m("apps.launch", Tier::Remote),
    m("open", Tier::Remote),
    m("windows.list", Tier::Viewer),
    m("windows.focus", Tier::Remote),
    m("windows.minimize", Tier::Remote),
    m("windows.maximize", Tier::Remote),
    m("windows.restore", Tier::Remote),
    m("windows.close", Tier::Remote),
];

const DEFAULT_ICON_SIZE: u64 = 64;

#[async_trait]
impl Module for AppsModule {
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
            "apps.list" => {
                let apps = blocking(move || p.list_apps()).await?;
                let list: Vec<Value> = apps
                    .into_iter()
                    .map(|a| {
                        json!({
                            "app_id": a.app_id,
                            "name": a.name,
                            "kind": match a.kind {
                                AppKind::Packaged => "packaged",
                                AppKind::Desktop => "desktop",
                            },
                        })
                    })
                    .collect();
                Ok(json!({"apps": list}))
            }
            "apps.icon" => {
                let app_id = str_param(&params, "app_id")?.to_string();
                let size = opt_u64(&params, "size")?.unwrap_or(DEFAULT_ICON_SIZE);
                if !(1..=1024).contains(&size) {
                    return Err(RpcError::bad_params("size must be 1-1024"));
                }
                let png = blocking(move || p.app_icon_png(&app_id, size as u32)).await?;
                Ok(json!({"png": util::b64_encode(&png)}))
            }
            "apps.launch" => {
                let app_id = str_param(&params, "app_id")?.to_string();
                let id = app_id.clone();
                blocking(move || p.launch_app(&id)).await?;
                crate::g5::record_launch(&ctx.state, &app_id).await;
                Ok(json!({}))
            }
            "open" => {
                let target = check_open_target(str_param(&params, "target")?)?;
                blocking(move || p.open_target(&target)).await?;
                Ok(json!({}))
            }
            "windows.list" => {
                let windows = blocking(move || p.list_windows()).await?;
                let list: Vec<Value> = windows
                    .into_iter()
                    .map(|w| {
                        json!({
                            "hwnd": w.hwnd,
                            "title": w.title,
                            "pid": w.pid,
                            "app_id": w.app_id,
                            "is_minimized": w.is_minimized,
                            "is_foreground": w.is_foreground,
                        })
                    })
                    .collect();
                Ok(json!({"windows": list}))
            }
            "windows.focus" => {
                let hwnd = u64_param(&params, "hwnd")?;
                let r = blocking(move || p.focus_window(hwnd)).await?;
                Ok(json!({"focused": r.focused, "method": r.method}))
            }
            "windows.minimize" | "windows.maximize" | "windows.restore" | "windows.close" => {
                let hwnd = u64_param(&params, "hwnd")?;
                let cmd = match method {
                    "windows.minimize" => WindowCommand::Minimize,
                    "windows.maximize" => WindowCommand::Maximize,
                    "windows.restore" => WindowCommand::Restore,
                    _ => WindowCommand::Close,
                };
                blocking(move || p.window_command(hwnd, cmd)).await?;
                Ok(json!({}))
            }
            _ => Err(RpcError::unknown_method(method)),
        }
    }
}
