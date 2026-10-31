//! `screen.monitors` / `screen.capture` backed by `helm-win-capture` (GDI stills).

use crate::platform::{Monitor, PResult};
use crate::router::RpcError;

#[cfg(windows)]
pub fn monitors() -> PResult<Vec<Monitor>> {
    Ok(helm_win_capture::monitors()
        .into_iter()
        .map(|m| Monitor {
            id: m.id,
            name: m.name,
            x: m.x,
            y: m.y,
            width: m.width,
            height: m.height,
            is_primary: m.is_primary,
        })
        .collect())
}

/// `None` = the primary monitor.
#[cfg(windows)]
pub fn screenshot_png(monitor_id: Option<&str>) -> PResult<Vec<u8>> {
    let id = match monitor_id {
        Some(id) => {
            if !helm_win_capture::monitors().iter().any(|m| m.id == id) {
                return Err(RpcError::bad_params(format!("no monitor with id {id:?}")));
            }
            id.to_string()
        }
        None => helm_win_capture::monitors()
            .into_iter()
            .find(|m| m.is_primary)
            .map(|m| m.id)
            .ok_or_else(|| RpcError::internal("no primary monitor found"))?,
    };
    helm_win_capture::screenshot_png(Some(&id)).map_err(|e| RpcError::internal(e.to_string()))
}

#[cfg(not(windows))]
pub fn monitors() -> PResult<Vec<Monitor>> {
    Err(RpcError::internal(
        "screen capture is only available on Windows",
    ))
}

#[cfg(not(windows))]
pub fn screenshot_png(_monitor_id: Option<&str>) -> PResult<Vec<u8>> {
    Err(RpcError::internal(
        "screen capture is only available on Windows",
    ))
}

// ---- Phase 2: live streams ----

#[cfg(windows)]
mod live {
    use helm_win_capture::{StreamConfig, StreamSource, Streamer};

    use crate::platform::{
        AccessUnitSink, AudioSink, AudioStream, PResult, Rect, VideoInfo, VideoParams,
        VideoSource, VideoStream,
    };
    use crate::router::RpcError;

    struct RealVideo {
        streamer: Streamer,
        source: VideoSource,
        /// Monitor rectangle at start (monitors do not move while streaming; a topology
        /// change ends the capture).
        monitor_rect: Option<Rect>,
        info: VideoInfo,
    }

    impl VideoStream for RealVideo {
        fn info(&self) -> VideoInfo {
            self.info.clone()
        }
        fn request_keyframe(&self) {
            self.streamer.request_keyframe();
        }
        fn set_bitrate(&self, kbps: u32) {
            self.streamer.set_bitrate(kbps);
        }
        fn is_running(&self) -> bool {
            self.streamer.is_running()
        }
        fn ended_reason(&self) -> Option<String> {
            self.streamer.stats().ended_reason
        }
        fn source_rect(&self) -> Option<Rect> {
            match self.source {
                VideoSource::Monitor(_) => self.monitor_rect,
                VideoSource::Window(hwnd) => crate::cursor::window_rect(hwnd),
            }
        }
    }

    fn monitor_rect(id: Option<&str>) -> Option<Rect> {
        helm_win_capture::monitors()
            .into_iter()
            .find(|m| match id {
                Some(id) => m.id == id,
                None => m.is_primary,
            })
            .map(|m| Rect {
                x: m.x,
                y: m.y,
                width: m.width,
                height: m.height,
            })
    }

    pub fn start_video(params: VideoParams, mut sink: AccessUnitSink) -> PResult<Box<dyn VideoStream>> {
        let source = match &params.source {
            VideoSource::Monitor(None) => StreamSource::PrimaryMonitor,
            VideoSource::Monitor(Some(id)) => StreamSource::Monitor(id.clone()),
            VideoSource::Window(hwnd) => StreamSource::Window(*hwnd as isize),
        };
        let config = StreamConfig {
            source,
            max_width: params.max_width,
            max_height: params.max_height,
            fps: params.fps,
            bitrate_kbps: params.bitrate_kbps,
            ..StreamConfig::default()
        };
        let monitor_rect = match &params.source {
            VideoSource::Monitor(id) => Some(
                monitor_rect(id.as_deref())
                    .ok_or_else(|| RpcError::bad_params("monitor not found"))?,
            ),
            VideoSource::Window(_) => None,
        };
        let streamer = Streamer::start(config, move |au, key, pts| sink(au, key, pts))
            .map_err(|e| match e {
                helm_win_capture::Error::InvalidConfig(m) => RpcError::bad_params(m),
                helm_win_capture::Error::MonitorNotFound(m) => {
                    RpcError::bad_params(format!("monitor not found: {m}"))
                }
                helm_win_capture::Error::Unsupported(m) => RpcError::unsupported(m),
                other => RpcError::internal(other.to_string()),
            })?;
        let i = streamer.info();
        let info = VideoInfo {
            width: i.width,
            height: i.height,
            fps: i.fps,
            hardware: i.hardware,
            encoder_name: i.encoder_name.clone(),
        };
        Ok(Box::new(RealVideo {
            streamer,
            source: params.source,
            monitor_rect,
            info,
        }))
    }

    struct RealAudio(helm_win_audio::AudioStreamer, u32);

    impl AudioStream for RealAudio {
        fn is_running(&self) -> bool {
            self.0.is_running()
        }
        fn pre_skip(&self) -> u32 {
            self.1
        }
    }

    pub fn start_audio(mut sink: AudioSink) -> PResult<Box<dyn AudioStream>> {
        let config = helm_win_audio::AudioConfig::default(); // 48 kHz stereo, 20 ms Opus
        // Same settings as the streamer's encoder, so this is its real look-ahead.
        let pre_skip = helm_win_audio::Encoder::new(&config)
            .and_then(|mut e| e.lookahead())
            .unwrap_or(120);
        helm_win_audio::AudioStreamer::start(config, move |packet, pts| sink(packet, pts))
            .map(|s| Box::new(RealAudio(s, pre_skip)) as Box<dyn AudioStream>)
            .map_err(|e| RpcError::unsupported(format!("system audio unavailable: {e}")))
    }
}

#[cfg(windows)]
pub use live::{start_audio, start_video};

#[cfg(not(windows))]
pub fn start_video(
    _params: crate::platform::VideoParams,
    _sink: crate::platform::AccessUnitSink,
) -> PResult<Box<dyn crate::platform::VideoStream>> {
    Err(RpcError::unsupported("streaming is only available on Windows"))
}

#[cfg(not(windows))]
pub fn start_audio(
    _sink: crate::platform::AudioSink,
) -> PResult<Box<dyn crate::platform::AudioStream>> {
    Err(RpcError::unsupported("audio is only available on Windows"))
}

/// Make the whole process per-monitor-v2 DPI aware (once, at startup) so `SendInput`
/// absolute moves, cursor positions and capture all use physical pixels.
pub fn make_process_dpi_aware() -> bool {
    #[cfg(windows)]
    {
        helm_win_capture::dpi::set_process_per_monitor_v2()
    }
    #[cfg(not(windows))]
    {
        false
    }
}
