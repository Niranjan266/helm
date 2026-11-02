//! `dock.get/set`, `deck.get/set/run`.

use std::time::Duration;

use async_trait::async_trait;
use serde_json::{Value, json};

use super::{check_open_target, internal, str_param, typed_param};
use crate::platform::{InputEvent, blocking};
use crate::router::{MethodSpec, Module, RequestContext, RpcError, Tier};
use crate::server::AppState;
use std::sync::Arc;
use crate::store::{self, DeckAction, DeckButton, DeckPage, Tile};

pub struct LayoutModule;

const fn m(name: &'static str, min_tier: Tier) -> MethodSpec {
    MethodSpec { name, min_tier }
}

const METHODS: &[MethodSpec] = &[
    m("dock.get", Tier::Viewer),
    m("dock.set", Tier::Remote),
    m("deck.get", Tier::Viewer),
    m("deck.set", Tier::Remote),
    m("deck.run", Tier::Remote),
];

#[async_trait]
impl Module for LayoutModule {
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
            "dock.get" => {
                let pages = blocking(move || state.layouts.dock().map_err(internal)).await?;
                // G5 phones ask for `v:2` (folders); older ones get folders flattened.
                if is_v2(&params) {
                    Ok(json!({"pages": pages, "v": 2}))
                } else {
                    Ok(json!({"pages": store::flatten_pages(&pages)}))
                }
            }
            "dock.set" => {
                let pages: Vec<Vec<Tile>> = typed_param(&params, "pages")?;
                store::validate_pages(&pages).map_err(RpcError::bad_params)?;
                let saved = pages.clone();
                let st = state.clone();
                blocking(move || st.layouts.set_dock(saved).map_err(internal)).await?;
                state.broadcast(
                    "dock.changed",
                    json!({"pages": store::flatten_pages(&pages)}),
                );
                Ok(json!({}))
            }
            "deck.get" => {
                if is_v2(&params) {
                    let (buttons, pages) =
                        blocking(move || state.layouts.deck_pages().map_err(internal)).await?;
                    return Ok(json!({"buttons": buttons, "pages": pages, "v": 2}));
                }
                let buttons = blocking(move || state.layouts.deck().map_err(internal)).await?;
                Ok(json!({"buttons": buttons}))
            }
            "deck.set" => {
                let buttons: Vec<DeckButton> = typed_param(&params, "buttons")?;
                store::validate_buttons(&buttons).map_err(RpcError::bad_params)?;
                let pages: Option<Vec<DeckPage>> = match params.get("pages") {
                    None | Some(Value::Null) => None,
                    Some(_) => Some(typed_param(&params, "pages")?),
                };
                if let Some(p) = &pages {
                    store::validate_deck_pages(p).map_err(RpcError::bad_params)?;
                }
                let saved = buttons.clone();
                let st = state.clone();
                blocking(move || {
                    match pages {
                        Some(p) => st.layouts.set_deck2(saved, p),
                        None => st.layouts.set_deck(saved),
                    }
                    .map_err(internal)
                })
                .await?;
                state.broadcast("deck.changed", json!({"buttons": buttons}));
                Ok(json!({}))
            }
            "deck.run" => {
                let id = str_param(&params, "id")?.to_string();
                let st = state.clone();
                let buttons = blocking(move || st.layouts.deck().map_err(internal)).await?;
                let button = buttons
                    .into_iter()
                    .find(|b| b.id == id)
                    .ok_or_else(|| RpcError::bad_params(format!("no deck button {id:?}")))?;
                run_action(&ctx.state, button.action).await?;
                Ok(json!({}))
            }
            _ => Err(RpcError::unknown_method(method)),
        }
    }
}

/// G5 request shape (`{"v":2}`): folders in `dock.get`, pages in `deck.get`.
fn is_v2(params: &Value) -> bool {
    params.get("v").and_then(Value::as_u64).is_some_and(|v| v >= 2)
}

/// Inject a short sequence with a fresh injector (released when done).
async fn inject(state: &Arc<AppState>, event: InputEvent) -> Result<(), RpcError> {
    let p = state.platform.clone();
    blocking(move || {
        let injector = p.new_injector()?;
        let r = injector.inject(&event);
        let _ = injector.release_all();
        r.map_err(|e| match e {
            helm_win_input::Error::BlockedByElevation => RpcError::internal(
                "blocked_by_elevation: the foreground window is elevated or a secure desktop is shown",
            ),
            other => RpcError::internal(other.to_string()),
        })
    })
    .await
}

/// Run a deck action (deck buttons, triggers, plugins).
pub(crate) fn run_action<'a>(
    state: &'a Arc<AppState>,
    action: DeckAction,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), RpcError>> + Send + 'a>> {
    Box::pin(run_action_inner(state, action))
}

async fn run_action_inner(state: &Arc<AppState>, action: DeckAction) -> Result<(), RpcError> {
    let p = state.platform.clone();
    match action {
        DeckAction::Macro(steps) => {
            for step in steps {
                run_action(state, step).await?;
            }
            Ok(())
        }
        DeckAction::Repeat(r) => {
            for _ in 0..r.times.min(store::MAX_MACRO_STEPS as u32) {
                run_action(state, (*r.action).clone()).await?;
            }
            Ok(())
        }
        DeckAction::Remote(name) => {
            if let Some(combo) = store::remote_hotkey(&name) {
                return run_action(state, DeckAction::Hotkey(combo.to_string())).await;
            }
            crate::obs::run_remote(&state.data_dir, &name).await
        }
        DeckAction::Ha(call) => crate::p5::ha_call(state, &call).await,
        DeckAction::Hotkey(combo) => {
            let keys = helm_win_input::parse_shortcut(&combo)
                .ok_or_else(|| RpcError::bad_params(format!("unknown hotkey {combo:?}")))?;
            inject(state, InputEvent::Shortcut { keys }).await
        }
        DeckAction::Text(text) => inject(state, InputEvent::Text { text }).await,
        DeckAction::Launch(app_id) => {
            let id = app_id.clone();
            blocking(move || p.launch_app(&id)).await?;
            crate::g5::record_launch(state, &app_id).await;
            Ok(())
        }
        DeckAction::Open(target) => {
            let target = check_open_target(&target)?;
            blocking(move || p.open_target(&target)).await
        }
        DeckAction::Media(action) => blocking(move || p.media_control(action)).await,
        DeckAction::VolumeStep(step) => {
            blocking(move || {
                let v = p.volume()?;
                p.set_volume((f64::from(v.level) + step).clamp(0.0, 1.0) as f32)
            })
            .await
        }
        DeckAction::DelayMs(ms) => {
            tokio::time::sleep(Duration::from_millis(ms.min(store::MAX_DELAY_MS))).await;
            Ok(())
        }
    }
}
