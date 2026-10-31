//! Phase 5 methods (PROTOCOL §13): `term.*`, `notify.push`, `plugins.*`, `triggers.*`, `ha.*`.
//! (`record.*` lives in the stream module.) Every `term.*` call and `ha.config.set` is audited.

use std::sync::{Arc, Weak};

use async_trait::async_trait;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::{internal, opt_bool, opt_str, str_param, u64_param};
use crate::p5::{self, ha::HaConfig, term};
use crate::platform::{ReplyBox, Toast, ToastAction, ToastEvent, blocking};
use crate::router::{MethodSpec, Module, RequestContext, RpcError, Tier};
use crate::server::AppState;
use crate::util;

pub struct ExtrasModule;

const fn m(name: &'static str, min_tier: Tier) -> MethodSpec {
    MethodSpec { name, min_tier }
}

const METHODS: &[MethodSpec] = &[
    m("term.open", Tier::Admin),
    m("term.write", Tier::Admin),
    m("term.resize", Tier::Admin),
    m("term.close", Tier::Admin),
    m("notify.push", Tier::Remote),
    m("plugins.list", Tier::Viewer),
    m("plugins.set", Tier::Admin),
    m("triggers.get", Tier::Viewer),
    m("triggers.set", Tier::Remote),
    m("ha.config.set", Tier::Admin),
    m("ha.states", Tier::Viewer),
];

/// Toast icon PNG limit (decoded).
pub const MAX_ICON_BYTES: usize = 512 * 1024;
pub const ICONS_DIR: &str = "notify-icons";
const PNG_MAGIC: &[u8] = &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

fn text(params: &Value, name: &str, max: usize, required: bool) -> Result<String, RpcError> {
    let v = if required {
        Some(str_param(params, name)?)
    } else {
        opt_str(params, name)?
    };
    let v = v.unwrap_or_default();
    if v.chars().count() > max {
        return Err(RpcError::bad_params(format!("{name} is longer than {max} characters")));
    }
    Ok(v.to_string())
}

fn audit(ctx: &RequestContext, event: &str, detail: &str) {
    ctx.state
        .audit
        .record(event, Some(&ctx.device_id), Some(&ctx.peer), detail);
}

fn decode_b64(params: &Value, name: &str, max: usize) -> Result<Vec<u8>, RpcError> {
    let s = str_param(params, name)?;
    let bytes = util::b64_decode(s)
        .ok_or_else(|| RpcError::bad_params(format!("{name} is not valid base64")))?;
    if bytes.len() > max {
        return Err(RpcError::bad_params(format!("{name} is larger than {max} bytes")));
    }
    Ok(bytes)
}

/// Build the toast for `notify.push` (icon written to `<data_dir>\notify-icons`).
pub fn build_toast(params: &Value, icons_dir: &std::path::Path) -> Result<Toast, RpcError> {
    let key = text(params, "key", 64, true)?;
    if key.trim().is_empty() || key.chars().any(char::is_control) {
        return Err(RpcError::bad_params("key must be 1-64 printable characters"));
    }
    let app = text(params, "app", 100, false)?;
    let title = text(params, "title", 500, true)?;
    if title.trim().is_empty() {
        return Err(RpcError::bad_params("title must not be empty"));
    }
    let body = text(params, "body", 4000, false)?;
    let actions = match params.get("actions") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(list)) => list
            .iter()
            .map(|a| {
                let id = a.get("id").and_then(Value::as_str).unwrap_or_default();
                let label = a.get("label").and_then(Value::as_str).unwrap_or_default();
                if id.is_empty() || id.len() > 64 || label.trim().is_empty() || label.len() > 64 {
                    return Err(RpcError::bad_params(
                        "each action needs an id and a label (1-64 characters)",
                    ));
                }
                if id == REPLY_ID {
                    return Err(RpcError::bad_params("action id \"reply\" is reserved"));
                }
                Ok(ToastAction {
                    id: id.into(),
                    label: label.into(),
                })
            })
            .collect::<Result<Vec<_>, _>>()?,
        Some(_) => return Err(RpcError::bad_params("actions must be an array")),
    };
    let reply = opt_bool(params, "reply")?.unwrap_or(false).then(|| ReplyBox {
        id: REPLY_ID.into(),
        placeholder: "Reply\u{2026}".into(),
        button_label: "Send".into(),
    });
    let image = match opt_str(params, "icon_png_b64")? {
        None => None,
        Some(_) => {
            let png = decode_b64(params, "icon_png_b64", MAX_ICON_BYTES)?;
            if !png.starts_with(PNG_MAGIC) {
                return Err(RpcError::bad_params("icon_png_b64 is not a PNG"));
            }
            std::fs::create_dir_all(icons_dir).map_err(internal)?;
            let name = hex::encode(&Sha256::digest(&png)[..12]);
            let path = icons_dir.join(format!("{name}.png"));
            if !path.exists() {
                util::atomic_write(&path, &png).map_err(internal)?;
            }
            Some(path)
        }
    };
    let toast = Toast {
        title,
        body,
        app_name: (!app.trim().is_empty()).then_some(app),
        image,
        actions,
        reply,
        tag: Some(key),
        group: Some("helm".into()),
        silent: false,
    };
    helm_win_notify::model::validate_toast(&toast).map_err(|e| RpcError::bad_params(e.to_string()))?;
    Ok(toast)
}

/// Action id of the inline reply button.
pub const REPLY_ID: &str = "reply";

/// Toast clicks → `notify.action{key,action_id,reply_text?}` to the device's sessions.
fn toast_events(
    state: Weak<AppState>,
    device_id: String,
    key: String,
) -> impl Fn(ToastEvent) + Send + Sync + 'static {
    move |ev| match ev {
        ToastEvent::Activated { action_id, reply } => {
            let Some(st) = state.upgrade() else { return };
            let mut data = json!({"key": key, "action_id": action_id});
            if let Some(r) = reply {
                data["reply_text"] = Value::String(r);
            }
            for s in st.sessions().into_iter().filter(|s| s.device_id == device_id) {
                s.send_event("notify.action", data.clone());
            }
        }
        ToastEvent::Failed { message } => tracing::warn!("toast {key} failed: {message}"),
        ToastEvent::Dismissed { .. } => {}
    }
}

#[async_trait]
impl Module for ExtrasModule {
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
        let p5 = &state.p5;
        match method {
            "term.open" => {
                let shell_name = str_param(&params, "shell")?;
                let shell = term::parse_shell(shell_name)?;
                let (cols, rows) = term::check_size(
                    u64_param(&params, "cols")?,
                    u64_param(&params, "rows")?,
                )?;
                let r = p5.terms.open(state, &ctx.session, shell, cols, rows).await;
                match &r {
                    Ok(id) => audit(ctx, "term.open", &format!("term_id={id} shell={shell_name} {cols}x{rows}")),
                    Err(e) => audit(ctx, "term.open", &format!("shell={shell_name} failed: {}", e.message)),
                }
                Ok(json!({"term_id": r?}))
            }
            "term.write" => {
                let id = str_param(&params, "term_id")?.to_string();
                let data = decode_b64(&params, "data_b64", term::MAX_WRITE)?;
                // Only the size is logged: the keystrokes may contain passwords.
                audit(ctx, "term.write", &format!("term_id={id} bytes={}", data.len()));
                p5.terms.write(&id, ctx.session.id, data).await?;
                Ok(json!({}))
            }
            "term.resize" => {
                let id = str_param(&params, "term_id")?.to_string();
                let (cols, rows) = term::check_size(
                    u64_param(&params, "cols")?,
                    u64_param(&params, "rows")?,
                )?;
                audit(ctx, "term.resize", &format!("term_id={id} {cols}x{rows}"));
                p5.terms.resize(&id, ctx.session.id, cols, rows).await?;
                Ok(json!({}))
            }
            "term.close" => {
                let id = str_param(&params, "term_id")?.to_string();
                audit(ctx, "term.close", &format!("term_id={id}"));
                p5.terms.close(&id, ctx.session.id).await?;
                Ok(json!({}))
            }
            "notify.push" => {
                let toast = build_toast(&params, &state.data_dir.join(ICONS_DIR))?;
                let key = toast.tag.clone().unwrap_or_default();
                let cb = toast_events(Arc::downgrade(state), ctx.device_id.clone(), key);
                let p = state.platform.clone();
                blocking(move || p.show_toast(&toast, Box::new(cb))).await?;
                Ok(json!({}))
            }
            "plugins.list" => {
                let st = state.clone();
                let plugins = blocking(move || Ok(st.p5.plugins.list())).await?;
                Ok(json!({"plugins": plugins}))
            }
            "plugins.set" => {
                let id = str_param(&params, "id")?.to_string();
                let enabled = opt_bool(&params, "enabled")?
                    .ok_or_else(|| RpcError::bad_params("enabled (true/false) is required"))?;
                let granted: Vec<String> = match params.get("granted") {
                    None | Some(Value::Null) => Vec::new(),
                    Some(v) => serde_json::from_value(v.clone()).map_err(|_| {
                        RpcError::bad_params("granted must be a list of capability strings")
                    })?,
                };
                audit(ctx, "plugins.set", &format!("id={id} enabled={enabled} granted={}", granted.join(",")));
                let st = state.clone();
                let plugins = blocking(move || {
                    st.p5
                        .plugins
                        .set(&st, &id, enabled, granted)
                        .map_err(RpcError::bad_params)?;
                    Ok(st.p5.plugins.list())
                })
                .await?;
                Ok(json!({"plugins": plugins}))
            }
            "triggers.get" => Ok(json!({"triggers": p5.triggers.get()})),
            "triggers.set" => {
                let list: Vec<Value> = match params.get("triggers") {
                    Some(Value::Array(a)) => a.clone(),
                    _ => return Err(RpcError::bad_params("triggers (array) is required")),
                };
                let st = state.clone();
                blocking(move || st.p5.triggers.set(list).map_err(RpcError::bad_params)).await?;
                Ok(json!({}))
            }
            "ha.config.set" => {
                let url = str_param(&params, "url")?.trim().to_string();
                let token = str_param(&params, "token")?.trim().to_string();
                let config = HaConfig { url, token };
                // Shape check (URL, token characters) without contacting the server.
                if let Err(e) = p5::ha::client(&config, state.platform.http()) {
                    audit(ctx, "ha.config.set", &format!("url={} rejected", config.url));
                    return Err(e);
                }
                audit(ctx, "ha.config.set", &format!("url={}", config.url));
                let st = state.clone();
                blocking(move || st.p5.set_ha_config(config).map_err(internal)).await?;
                Ok(json!({}))
            }
            "ha.states" => Ok(json!({"states": p5::ha_states(state).await?})),
            _ => Err(RpcError::unknown_method(method)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toast_params_are_checked() {
        let dir = tempfile::tempdir().unwrap();
        let png = crate::platform::fake::TINY_PNG;
        let t = build_toast(
            &json!({"app":"WhatsApp","title":"Alice","body":"Lunch?","key":"msg-1",
                    "actions":[{"id":"read","label":"Mark read"}],"reply":true,
                    "icon_png_b64": util::b64_encode(png)}),
            dir.path(),
        )
        .unwrap();
        assert_eq!(t.tag.as_deref(), Some("msg-1"));
        assert_eq!(t.app_name.as_deref(), Some("WhatsApp"));
        assert_eq!(t.reply.as_ref().unwrap().id, REPLY_ID);
        let icon = t.image.unwrap();
        assert!(icon.starts_with(dir.path()));
        assert_eq!(std::fs::read(icon).unwrap(), png);
        for bad in [
            json!({"title":"x"}),
            json!({"title":"","key":"k"}),
            json!({"title":"x","key":"k","icon_png_b64":"not base64!"}),
            json!({"title":"x","key":"k","icon_png_b64": util::b64_encode(b"GIF89a")}),
            json!({"title":"x","key":"k","actions":[{"id":"","label":"a"}]}),
            json!({"title":"x","key":"k","actions":[{"id":"reply","label":"a"}]}),
            json!({"title":"x","key":"k","actions":"nope"}),
            json!({"title":"x","key":"k","actions":[
                {"id":"1","label":"a"},{"id":"2","label":"a"},{"id":"3","label":"a"},
                {"id":"4","label":"a"},{"id":"5","label":"a"}],"reply":true}),
            json!({"title":"x","key": "k".repeat(65)}),
        ] {
            assert_eq!(build_toast(&bad, dir.path()).unwrap_err().code, "bad_params", "{bad}");
        }
    }
}
