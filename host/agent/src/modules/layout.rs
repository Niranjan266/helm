//! `dock.get/set`, `deck.get/set/run`.

use std::time::Duration;

use async_trait::async_trait;
use serde_json::{Value, json};

use super::{check_open_target, internal, str_param, typed_param};
use crate::platform::{InputEvent, blocking};
use crate::router::{MethodSpec, Module, RequestContext, RpcError, Tier};
use crate::store::{self, DeckAction, DeckButton, Tile};

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
                Ok(json!({"pages": pages}))
            }
            "dock.set" => {
                let pages: Vec<Vec<Tile>> = typed_param(&params, "pages")?;
                store::validate_pages(&pages).map_err(RpcError::bad_params)?;
                let saved = pages.clone();
                let st = state.clone();
                blocking(move || st.layouts.set_dock(saved).map_err(internal)).await?;
                state.broadcast("dock.changed", json!({"pages": pages}));
                Ok(json!({}))
            }
            "deck.get" => {
                let buttons = blocking(move || state.layouts.deck().map_err(internal)).await?;
                Ok(json!({"buttons": buttons}))
            }
            "deck.set" => {
                let buttons: Vec<DeckButton> = typed_param(&params, "buttons")?;
                store::validate_buttons(&buttons).map_err(RpcError::bad_params)?;
                let saved = buttons.clone();
                let st = state.clone();
                blocking(move || st.layouts.set_deck(saved).map_err(internal)).await?;
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
                run_action(ctx, button.action).await?;
                Ok(json!({}))
            }
            _ => Err(RpcError::unknown_method(method)),
        }
    }
}

/// Inject a short sequence with a fresh injector (released when done).
async fn inject(ctx: &RequestContext, event: InputEvent) -> Result<(), RpcError> {
    let p = ctx.state.platform.clone();
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

pub(crate) fn run_action<'a>(
    ctx: &'a RequestContext,
    action: DeckAction,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), RpcError>> + Send + 'a>> {
    Box::pin(run_action_inner(ctx, action))
}

async fn run_action_inner(ctx: &RequestContext, action: DeckAction) -> Result<(), RpcError> {
    let p = ctx.state.platform.clone();
    match action {
        DeckAction::Macro(steps) => {
            for step in steps {
                run_action(ctx, step).await?;
            }
            Ok(())
        }
        DeckAction::Repeat(r) => {
            for _ in 0..r.times.min(store::MAX_MACRO_STEPS as u32) {
                run_action(ctx, (*r.action).clone()).await?;
            }
            Ok(())
        }
        DeckAction::Remote(name) => {
            if let Some(combo) = store::remote_hotkey(&name) {
                return run_action(ctx, DeckAction::Hotkey(combo.to_string())).await;
            }
            crate::obs::run_remote(&ctx.state.data_dir, &name).await
        }
        DeckAction::Hotkey(combo) => {
            let keys = helm_win_input::parse_shortcut(&combo)
                .ok_or_else(|| RpcError::bad_params(format!("unknown hotkey {combo:?}")))?;
            inject(ctx, InputEvent::Shortcut { keys }).await
        }
        DeckAction::Text(text) => inject(ctx, InputEvent::Text { text }).await,
        DeckAction::Launch(app_id) => blocking(move || p.launch_app(&app_id)).await,
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
