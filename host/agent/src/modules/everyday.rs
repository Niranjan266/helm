//! g6 methods (Docs/protocol/everyday.md): `clipboard.history*`, `clipboard.pick`,
//! `file.drop`, `phone.list`, `phone.ring`, `pc.notify.watch`, `schedule.*`, `system.health`.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use serde_json::{Value, json};

use super::{internal, opt_bool, opt_str, str_param, u64_param};
use crate::g6::{self, clip, schedule};
use crate::platform::blocking;
use crate::router::{MethodSpec, Module, RequestContext, RpcError, Tier};
use crate::util;

pub struct EverydayModule;

const fn m(name: &'static str, min_tier: Tier) -> MethodSpec {
    MethodSpec { name, min_tier }
}

const METHODS: &[MethodSpec] = &[
    m("clipboard.history", Tier::Remote),
    m("clipboard.history.set", Tier::Remote),
    m("clipboard.history.clear", Tier::Remote),
    m("clipboard.history.delete", Tier::Remote),
    m("clipboard.pick", Tier::Remote),
    m("file.drop", Tier::Remote),
    m("phone.list", Tier::Viewer),
    m("phone.ring", Tier::Remote),
    m("pc.notify.watch", Tier::Remote),
    m("schedule.list", Tier::Viewer),
    m("schedule.set", Tier::Remote),
    m("system.health", Tier::Viewer),
];

/// Never opened by `file.drop{mode:"open"}` (falls back to reveal).
pub const RISKY_EXTENSIONS: &[&str] = &[
    "exe", "com", "scr", "bat", "cmd", "ps1", "psm1", "vbs", "vbe", "js", "jse", "wsf", "wsh",
    "msi", "msp", "lnk", "url", "hta", "cpl", "reg", "jar",
];

pub fn is_risky(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| RISKY_EXTENSIONS.iter().any(|r| r.eq_ignore_ascii_case(e)))
}

/// `path` must be a regular file directly inside `downloads` (after resolving both).
pub fn check_drop_path(path: &str, downloads: &Path) -> Result<PathBuf, RpcError> {
    let bad = || RpcError::bad_params("path must be a file received with file.upload");
    let p = Path::new(path);
    if !p.is_absolute() {
        return Err(bad());
    }
    let file = std::fs::canonicalize(p).map_err(|_| bad())?;
    let dir = std::fs::canonicalize(downloads).map_err(|_| bad())?;
    if file.parent() != Some(dir.as_path()) || !std::fs::symlink_metadata(&file).map_err(|_| bad())?.is_file() {
        return Err(bad());
    }
    // Hand the platform the plain path (canonicalize adds a \\?\ prefix on Windows).
    Ok(p.to_path_buf())
}

fn audit(ctx: &RequestContext, event: &str, detail: &str) {
    ctx.state
        .audit
        .record(event, Some(&ctx.device_id), Some(&ctx.peer), detail);
}

fn item_id(params: &Value) -> Result<u64, RpcError> {
    u64_param(params, "id")
}

#[async_trait]
impl Module for EverydayModule {
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
        let hist = &state.g6.clipboard;
        match method {
            "clipboard.history" => Ok(hist.to_json()),
            "clipboard.history.set" => {
                let enabled = opt_bool(&params, "enabled")?;
                let persist = opt_bool(&params, "persist")?;
                audit(ctx, "clipboard_history", &format!("enabled={enabled:?} persist={persist:?}"));
                let st = state.clone();
                blocking(move || st.g6.clipboard.set_settings(enabled, persist).map_err(internal))
                    .await?;
                Ok(hist.to_json())
            }
            "clipboard.history.clear" => {
                hist.clear();
                state.broadcast("clipboard.history.changed", json!({}));
                Ok(json!({}))
            }
            "clipboard.history.delete" => {
                if !hist.delete(item_id(&params)?) {
                    return Err(RpcError::bad_params("no clipboard item with that id"));
                }
                state.broadcast("clipboard.history.changed", json!({}));
                Ok(json!({}))
            }
            "clipboard.pick" => {
                let id = item_id(&params)?;
                let to = str_param(&params, "to")?;
                let item = hist
                    .get(id)
                    .ok_or_else(|| RpcError::bad_params("no clipboard item with that id"))?;
                match to {
                    "phone" => Ok(match item.content {
                        clip::Content::Text(t) => json!({"text": t}),
                        clip::Content::Image(png) => json!({"png": util::b64_encode(&png)}),
                    }),
                    "pc" => {
                        let p = state.platform.clone();
                        blocking(move || match item.content {
                            clip::Content::Text(t) => p.set_clipboard_text(&t),
                            clip::Content::Image(png) => p.set_clipboard_png(&png),
                        })
                        .await?;
                        if hist.touch(id) {
                            state.broadcast("clipboard.history.changed", json!({}));
                        }
                        Ok(json!({}))
                    }
                    _ => Err(RpcError::bad_params("to must be \"phone\" or \"pc\"")),
                }
            }
            "file.drop" => {
                let path = str_param(&params, "path")?.to_string();
                let mode = str_param(&params, "mode")?;
                if !matches!(mode, "open" | "save" | "reveal") {
                    return Err(RpcError::bad_params("mode must be open, save or reveal"));
                }
                let file = check_drop_path(&path, &state.downloads_dir)?;
                let done = if mode == "open" && is_risky(&file) { "reveal" } else { mode };
                audit(ctx, "file_drop", &format!("mode={done} path={path}"));
                let p = state.platform.clone();
                let target = file.clone();
                match done {
                    "open" => {
                        let t = target.to_string_lossy().into_owned();
                        blocking(move || p.open_target(&t)).await?
                    }
                    "reveal" => blocking(move || p.reveal_in_explorer(&target)).await?,
                    _ => {}
                }
                Ok(json!({"path": path, "mode": done}))
            }
            "phone.list" => {
                let st = state.clone();
                let devices = blocking(move || st.devices.list().map_err(internal)).await?;
                let online: std::collections::HashSet<String> =
                    state.sessions().into_iter().map(|s| s.device_id.clone()).collect();
                let phones: Vec<Value> = devices
                    .iter()
                    .map(|d| {
                        json!({"device_id": d.device_id, "device_name": d.device_name,
                               "online": online.contains(&d.device_id),
                               "this": d.device_id == ctx.device_id})
                    })
                    .collect();
                Ok(json!({"phones": phones}))
            }
            "phone.ring" => {
                let target = opt_str(&params, "device_id")?.map(str::to_string);
                let stop = opt_bool(&params, "stop")?.unwrap_or(false);
                let name = state
                    .devices
                    .list()
                    .ok()
                    .and_then(|l| l.into_iter().find(|d| d.device_id == ctx.device_id))
                    .map(|d| d.device_name)
                    .unwrap_or_default();
                let from = json!({"device_id": ctx.device_id, "name": name});
                let n = g6::ring_phones(state, target.as_deref(), Some(&ctx.device_id), from, stop);
                audit(
                    ctx,
                    "phone_ring",
                    &format!("target={} stop={stop} sessions={n}", target.as_deref().unwrap_or("others")),
                );
                Ok(json!({"rang": n}))
            }
            "pc.notify.watch" => {
                let on = opt_bool(&params, "on")?
                    .ok_or_else(|| RpcError::bad_params("on (bool) is required"))?;
                let access = g6::notify_watch(state, ctx.session.id, on).await?;
                Ok(json!({"access": access}))
            }
            "schedule.list" => Ok(json!({"schedules": g6::schedules_json(state)})),
            "schedule.set" => {
                let raw = match params.get("schedules") {
                    Some(Value::Array(a)) => a.clone(),
                    _ => return Err(RpcError::bad_params("schedules (array) is required")),
                };
                let mut list = Vec::with_capacity(raw.len());
                for (i, v) in raw.into_iter().enumerate() {
                    let mut v = v;
                    if let Some(o) = v.as_object_mut() {
                        o.remove("next_ms");
                        o.remove("last_run_ms");
                    }
                    list.push(
                        serde_json::from_value::<schedule::Schedule>(v)
                            .map_err(|e| RpcError::bad_params(format!("schedules[{i}]: {e}")))?,
                    );
                }
                if list.iter().any(schedule::Schedule::is_power) && ctx.tier < Tier::Admin {
                    return Err(RpcError::forbidden("power schedules need tier admin"));
                }
                let offset = state.platform.utc_offset_minutes();
                schedule::validate(&mut list, g6::now_ms(), offset).map_err(RpcError::bad_params)?;
                audit(
                    ctx,
                    "schedule_set",
                    &format!(
                        "count={} power={}",
                        list.len(),
                        list.iter().filter(|s| s.is_power()).count()
                    ),
                );
                let st = state.clone();
                blocking(move || st.g6.schedules.set(list).map_err(internal)).await?;
                g6::broadcast_schedules(state);
                Ok(json!({"schedules": g6::schedules_json(state)}))
            }
            "system.health" => g6::health(state).await,
            _ => Err(RpcError::unknown_method(method)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn risky_extensions() {
        assert!(is_risky(Path::new(r"C:\d\setup.EXE")));
        assert!(is_risky(Path::new("x.ps1")));
        assert!(!is_risky(Path::new("photo.jpg")));
        assert!(!is_risky(Path::new("noext")));
    }

    #[test]
    fn drop_path_must_be_a_file_in_downloads() {
        let dir = tempfile::tempdir().unwrap();
        let downloads = dir.path().join("dl");
        std::fs::create_dir_all(downloads.join("sub")).unwrap();
        std::fs::write(downloads.join("a.txt"), b"x").unwrap();
        std::fs::write(downloads.join("sub").join("b.txt"), b"x").unwrap();
        std::fs::write(dir.path().join("outside.txt"), b"x").unwrap();
        let ok = downloads.join("a.txt");
        assert_eq!(check_drop_path(ok.to_str().unwrap(), &downloads).unwrap(), ok);
        for bad in [
            downloads.join("sub").join("b.txt"),
            downloads.join("sub"),
            dir.path().join("outside.txt"),
            downloads.join("..").join("outside.txt"),
            downloads.join("missing.txt"),
        ] {
            assert!(check_drop_path(bad.to_str().unwrap(), &downloads).is_err(), "{bad:?}");
        }
        assert!(check_drop_path("a.txt", &downloads).is_err());
    }
}
