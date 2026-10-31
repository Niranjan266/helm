//! `stream.start` / `stop` / `idr` / `config` / `stats`, `record.start` / `stop` (PROTOCOL §10).

use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde_json::{Value, json};

use super::{opt_bool, opt_str, opt_u64};
use crate::platform::{VideoParams, VideoSource};
use crate::router::{MethodSpec, Module, RequestContext, RpcError, Tier};
use crate::server::AppState;
use crate::stream::ActiveStream;

pub struct StreamModule;

const METHODS: &[MethodSpec] = &[
    MethodSpec {
        name: "stream.start",
        min_tier: Tier::Remote,
    },
    MethodSpec {
        name: "stream.stop",
        min_tier: Tier::Remote,
    },
    MethodSpec {
        name: "stream.idr",
        min_tier: Tier::Remote,
    },
    MethodSpec {
        name: "stream.config",
        min_tier: Tier::Remote,
    },
    MethodSpec {
        name: "stream.stats",
        min_tier: Tier::Remote,
    },
    MethodSpec {
        name: "record.start",
        min_tier: Tier::Remote,
    },
    MethodSpec {
        name: "record.stop",
        min_tier: Tier::Remote,
    },
];

pub const MIN_DIMENSION: u64 = 64;
pub const MAX_DIMENSION: u64 = 4096;
pub const MAX_FPS: u64 = 240;
pub const MIN_BITRATE_KBPS: u64 = 100;
pub const MAX_BITRATE_KBPS: u64 = 200_000;

fn ranged(params: &Value, name: &str, default: u64, min: u64, max: u64) -> Result<u32, RpcError> {
    let v = opt_u64(params, name)?.unwrap_or(default);
    if !(min..=max).contains(&v) {
        return Err(RpcError::bad_params(format!("{name} must be {min}..={max}")));
    }
    Ok(v as u32)
}

fn opt_ranged(params: &Value, name: &str, min: u64, max: u64) -> Result<Option<u32>, RpcError> {
    match opt_u64(params, name)? {
        None => Ok(None),
        Some(v) if (min..=max).contains(&v) => Ok(Some(v as u32)),
        Some(_) => Err(RpcError::bad_params(format!("{name} must be {min}..={max}"))),
    }
}

/// Parse `stream.start` params (defaults: primary monitor, 1920×1080, 60 fps, 8000 kbit/s).
pub fn parse_start(params: &Value) -> Result<(VideoParams, bool), RpcError> {
    let monitor = opt_str(params, "monitor_id")?;
    let hwnd = opt_u64(params, "hwnd")?;
    let source = match (monitor, hwnd) {
        (Some(_), Some(_)) => {
            return Err(RpcError::bad_params("give monitor_id or hwnd, not both"));
        }
        (_, Some(h)) => VideoSource::Window(h),
        (m, None) => VideoSource::Monitor(m.map(str::to_string)),
    };
    let video = VideoParams {
        source,
        max_width: ranged(params, "max_width", 1920, MIN_DIMENSION, MAX_DIMENSION)?,
        max_height: ranged(params, "max_height", 1080, MIN_DIMENSION, MAX_DIMENSION)?,
        fps: ranged(params, "fps", 60, 1, MAX_FPS)?,
        bitrate_kbps: ranged(
            params,
            "bitrate_kbps",
            8_000,
            MIN_BITRATE_KBPS,
            MAX_BITRATE_KBPS,
        )?,
    };
    Ok((video, opt_bool(params, "audio")?.unwrap_or(false)))
}

fn no_stream() -> RpcError {
    RpcError::bad_params("no stream is running (call stream.start)")
}

fn revoke_token(state: &AppState, stream: &Arc<ActiveStream>) {
    let weak = Arc::downgrade(stream);
    state.lock_stream_tokens().revoke_where(|w| w.ptr_eq(&weak));
}

/// Ends a stream nobody opened in time, or whose capture stopped by itself.
fn spawn_watchdog(state: Arc<AppState>, weak: Weak<ActiveStream>, lifetime: Duration) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_millis(250));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            let Some(s) = weak.upgrade() else { break };
            if s.is_stopped() {
                break;
            }
            if !s.is_attached() && s.age() >= lifetime {
                state.lock_stream_tokens().revoke_where(|w| w.ptr_eq(&weak));
                s.end("the stream socket was not opened in time");
                break;
            }
            let (running, reason) = s.video_state();
            if !running {
                s.end(reason.as_deref().unwrap_or("capture stopped"));
                break;
            }
        }
    });
}

#[async_trait]
impl Module for StreamModule {
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
        let session = &ctx.session;
        match method {
            "stream.start" => {
                let (video, want_audio) = parse_start(&params)?;
                // One stream per control session: a new start replaces the old one.
                if let Some(old) = session.take_stream() {
                    revoke_token(state, &old);
                    old.stop();
                }
                let id = state.next_stream_id();
                let (stream, pre_skip) =
                    ActiveStream::start(id, state.platform.clone(), session, video, want_audio)
                        .await?;
                let token = state.lock_stream_tokens().issue(
                    &ctx.device_id,
                    Arc::downgrade(&stream),
                    Instant::now(),
                );
                if let Some(raced) = session.set_stream(stream.clone()) {
                    revoke_token(state, &raced);
                    raced.stop();
                }
                spawn_watchdog(
                    state.clone(),
                    Arc::downgrade(&stream),
                    state.stream_token_lifetime,
                );
                let audio = pre_skip.is_some();
                let info = stream.info();
                tracing::info!(
                    "stream {id} started: {}x{}@{} {} ({}), audio={audio}",
                    info.width,
                    info.height,
                    info.fps,
                    info.encoder_name,
                    if info.hardware { "hardware" } else { "software" }
                );
                Ok(json!({
                    "token": token,
                    "width": info.width,
                    "height": info.height,
                    "fps": info.fps,
                    "codec": "h264",
                    "encoder": if info.hardware { "hardware" } else { "software" },
                    "audio": audio,
                    "opus_pre_skip": pre_skip,
                }))
            }
            "stream.stop" => {
                if let Some(s) = session.take_stream() {
                    revoke_token(state, &s);
                    s.stop();
                }
                Ok(json!({}))
            }
            "stream.idr" => {
                session.stream().ok_or_else(no_stream)?.request_keyframe();
                Ok(json!({}))
            }
            "stream.config" => {
                let bitrate = opt_ranged(&params, "bitrate_kbps", MIN_BITRATE_KBPS, MAX_BITRATE_KBPS)?;
                let fps = opt_ranged(&params, "fps", 1, MAX_FPS)?;
                let stream = session.stream().ok_or_else(no_stream)?;
                stream.reconfigure(bitrate, fps).await
            }
            "stream.stats" => {
                let dropped = opt_u64(&params, "dropped")?.unwrap_or(0);
                match session.stream() {
                    Some(s) => Ok(s.on_stats(dropped)),
                    None => Ok(json!({})),
                }
            }
            "record.start" => {
                let audio = opt_bool(&params, "audio")?.unwrap_or(false);
                let path = crate::p5::record::start(state, session, audio).await?;
                Ok(json!({"path": path}))
            }
            "record.stop" => {
                let file = crate::p5::record::stop(state, session.id).await?;
                Ok(json!({"path": file.path, "duration_ms": file.duration_ms}))
            }
            _ => Err(RpcError::unknown_method(method)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_params_defaults_and_ranges() {
        let (v, audio) = parse_start(&json!({})).unwrap();
        assert_eq!(v.source, VideoSource::Monitor(None));
        assert_eq!((v.max_width, v.max_height, v.fps, v.bitrate_kbps), (1920, 1080, 60, 8000));
        assert!(!audio);
        let (v, audio) =
            parse_start(&json!({"hwnd": 42, "fps": 30, "audio": true, "max_width": 1280})).unwrap();
        assert_eq!(v.source, VideoSource::Window(42));
        assert_eq!((v.max_width, v.fps), (1280, 30));
        assert!(audio);
        for bad in [
            json!({"monitor_id": "x", "hwnd": 1}),
            json!({"fps": 0}),
            json!({"fps": 1000}),
            json!({"max_width": 10}),
            json!({"bitrate_kbps": 50}),
            json!({"audio": "yes"}),
            json!({"monitor_id": 3}),
        ] {
            assert_eq!(parse_start(&bad).unwrap_err().code, "bad_params", "{bad}");
        }
    }
}
