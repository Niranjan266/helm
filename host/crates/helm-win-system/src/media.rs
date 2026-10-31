//! Now-playing media (System Media Transport Controls).
//!
//! Backed by WinRT `Windows.Media.Control.GlobalSystemMediaTransportControlsSessionManager`.
//! Works with any app that integrates with the Windows media flyout (Spotify,
//! browsers, Groove/Media Player, VLC with SMTC, ...).

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::Result;

/// Playback status of the current media session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlaybackStatus {
    /// The app closed its media session.
    Closed,
    /// The session is open but nothing is loaded.
    Opened,
    /// The app is changing tracks.
    Changing,
    /// Stopped.
    Stopped,
    /// Playing.
    Playing,
    /// Paused.
    Paused,
    /// Status not reported by the OS.
    Unknown,
}

/// Snapshot of the current media session.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MediaState {
    /// Source app id (AppUserModelID, e.g. `Spotify.exe` or `MSEdge`).
    pub app_id: String,
    /// Track title (may be empty).
    pub title: String,
    /// Artist (may be empty).
    pub artist: String,
    /// Album title (may be empty).
    pub album: String,
    /// Playback status.
    pub status: PlaybackStatus,
    /// Playback position in ms, extrapolated to "now" while playing.
    /// `None` when the app does not publish a timeline.
    pub position_ms: Option<i64>,
    /// Track length in ms, `None` when the app does not publish a timeline.
    pub duration_ms: Option<i64>,
    /// Playback rate (1.0 = normal).
    pub rate: f64,
    /// Whether a thumbnail is available via [`media_artwork`].
    pub has_artwork: bool,
    /// Play is allowed.
    pub can_play: bool,
    /// Pause is allowed.
    pub can_pause: bool,
    /// Next track is allowed.
    pub can_next: bool,
    /// Previous track is allowed.
    pub can_prev: bool,
    /// Stop is allowed.
    pub can_stop: bool,
    /// Seeking is allowed.
    pub can_seek: bool,
}

/// A transport command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaAction {
    /// Play.
    Play,
    /// Pause.
    Pause,
    /// Toggle play/pause.
    TogglePlayPause,
    /// Next track.
    Next,
    /// Previous track.
    Previous,
    /// Stop.
    Stop,
    /// Seek to an absolute position in milliseconds.
    SeekMs(i64),
}

/// Album artwork bytes as delivered by the app.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Artwork {
    /// MIME type, e.g. `image/png` or `image/jpeg`.
    pub mime: String,
    /// Encoded image bytes.
    pub data: Vec<u8>,
}

/// Snapshot of the current media session, or `None` when no app is playing.
pub fn media_state() -> Result<Option<MediaState>> {
    imp::media_state()
}

/// Artwork of the current track exactly as the app provides it (usually PNG or JPEG).
/// `None` when there is no session or no thumbnail.
pub fn media_artwork() -> Result<Option<Artwork>> {
    imp::media_artwork()
}

/// Artwork of the current track as PNG bytes. If the app already supplies a
/// PNG it is returned unchanged; otherwise it is transcoded (WinRT
/// `BitmapDecoder` / `BitmapEncoder`).
pub fn media_artwork_png() -> Result<Option<Vec<u8>>> {
    imp::media_artwork_png()
}

/// Send a transport command to the current session.
///
/// Returns [`crate::Error::NotFound`] when there is no session and
/// [`crate::Error::Rejected`] when the app refuses the command.
pub fn media_control(action: MediaAction) -> Result<()> {
    imp::media_control(action)
}

/// Default debounce for [`MediaWatcher`].
pub const DEFAULT_DEBOUNCE: Duration = Duration::from_millis(250);

/// Watches the current session, its media properties, playback info and
/// timeline, and calls back with the new state (debounced).
///
/// The callback runs on the watcher's own thread. It is called once right
/// after start with the initial state, then whenever something changes.
/// Pure position drift during playback is suppressed (a seek still fires).
/// Dropping the watcher stops it.
pub struct MediaWatcher {
    _inner: imp::WatcherInner,
}

impl MediaWatcher {
    /// Start watching with [`DEFAULT_DEBOUNCE`].
    pub fn start<F>(callback: F) -> Result<MediaWatcher>
    where
        F: Fn(Option<MediaState>) + Send + 'static,
    {
        Self::start_with_debounce(callback, DEFAULT_DEBOUNCE)
    }

    /// Start watching with a custom debounce window.
    pub fn start_with_debounce<F>(callback: F, debounce: Duration) -> Result<MediaWatcher>
    where
        F: Fn(Option<MediaState>) + Send + 'static,
    {
        Ok(MediaWatcher { _inner: imp::WatcherInner::start(Box::new(callback), debounce)? })
    }
}

/// True when `new` differs from `prev` in a way worth reporting.
/// Position changes that match normal playback progress (within 1.5 s of the
/// expected position after `elapsed_ms`) are not significant.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn significant_change(prev: &Option<MediaState>, elapsed_ms: i64, new: &Option<MediaState>) -> bool {
    match (prev, new) {
        (None, None) => false,
        (Some(a), Some(b)) => {
            let mut a2 = a.clone();
            a2.position_ms = None;
            let mut b2 = b.clone();
            b2.position_ms = None;
            if a2 != b2 {
                return true;
            }
            match (a.position_ms, b.position_ms) {
                (Some(pa), Some(pb)) => {
                    let expected = if a.status == PlaybackStatus::Playing {
                        pa + (elapsed_ms as f64 * a.rate) as i64
                    } else {
                        pa
                    };
                    (pb - expected).abs() > 1500
                }
                (None, None) => false,
                _ => true,
            }
        }
        _ => true,
    }
}

type Callback = Box<dyn Fn(Option<MediaState>) + Send + 'static>;

#[cfg(windows)]
mod imp {
    use std::cell::RefCell;
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::sync::mpsc;
    use std::thread::{self, JoinHandle};
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    use windows::Foundation::TypedEventHandler;
    use windows::Graphics::Imaging::{BitmapDecoder, BitmapEncoder};
    use windows::Media::Control::{
        GlobalSystemMediaTransportControlsSession as Session,
        GlobalSystemMediaTransportControlsSessionManager as Manager,
        GlobalSystemMediaTransportControlsSessionPlaybackStatus as WinStatus,
    };
    use windows::Storage::Streams::{DataReader, IRandomAccessStreamReference, InMemoryRandomAccessStream};

    use super::{Artwork, Callback, MediaAction, MediaState, PlaybackStatus, significant_change};
    use crate::worker::{ComApartment, static_worker};
    use crate::{Error, Result};

    static_worker!(worker, "helm-media");

    thread_local! {
        static MANAGER: RefCell<Option<Manager>> = const { RefCell::new(None) };
    }

    /// Cached manager for the media worker thread.
    fn with_manager<R>(f: impl FnOnce(&Manager) -> Result<R>) -> Result<R> {
        let mgr = MANAGER.with(|m| -> Result<Manager> {
            if let Some(mgr) = m.borrow().as_ref() {
                return Ok(mgr.clone());
            }
            let mgr = Manager::RequestAsync()?.join()?;
            *m.borrow_mut() = Some(mgr.clone());
            Ok(mgr)
        })?;
        let r = f(&mgr);
        if matches!(r, Err(Error::Os { .. })) {
            MANAGER.with(|m| *m.borrow_mut() = None);
        }
        r
    }

    fn current_session(mgr: &Manager) -> Option<Session> {
        mgr.GetCurrentSession().ok()
    }

    const TICKS_PER_MS: i64 = 10_000;
    /// 100 ns ticks between 1601-01-01 and 1970-01-01.
    const EPOCH_DIFF_TICKS: i64 = 116_444_736_000_000_000;

    fn now_ticks() -> i64 {
        let d = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
        EPOCH_DIFF_TICKS + (d.as_nanos() / 100) as i64
    }

    fn map_status(s: WinStatus) -> PlaybackStatus {
        match s {
            WinStatus::Closed => PlaybackStatus::Closed,
            WinStatus::Opened => PlaybackStatus::Opened,
            WinStatus::Changing => PlaybackStatus::Changing,
            WinStatus::Stopped => PlaybackStatus::Stopped,
            WinStatus::Playing => PlaybackStatus::Playing,
            WinStatus::Paused => PlaybackStatus::Paused,
            _ => PlaybackStatus::Unknown,
        }
    }

    pub(super) fn read_state(mgr: &Manager) -> Result<Option<MediaState>> {
        let Some(session) = current_session(mgr) else { return Ok(None) };
        let app_id = session.SourceAppUserModelId().map(|s| s.to_string()).unwrap_or_default();

        let (mut title, mut artist, mut album, mut has_artwork) = (String::new(), String::new(), String::new(), false);
        if let Ok(props) = session.TryGetMediaPropertiesAsync().and_then(|op| op.join()) {
            title = props.Title().map(|s| s.to_string()).unwrap_or_default();
            artist = props.Artist().map(|s| s.to_string()).unwrap_or_default();
            album = props.AlbumTitle().map(|s| s.to_string()).unwrap_or_default();
            has_artwork = props.Thumbnail().is_ok();
        }

        let info = session.GetPlaybackInfo()?;
        let status = info.PlaybackStatus().map(map_status).unwrap_or(PlaybackStatus::Unknown);
        let rate = info.PlaybackRate().and_then(|r| r.Value()).unwrap_or(1.0);
        let (mut can_play, mut can_pause, mut can_next, mut can_prev, mut can_stop, mut can_seek) =
            (false, false, false, false, false, false);
        if let Ok(c) = info.Controls() {
            can_play = c.IsPlayEnabled().unwrap_or(false);
            can_pause = c.IsPauseEnabled().unwrap_or(false);
            can_next = c.IsNextEnabled().unwrap_or(false);
            can_prev = c.IsPreviousEnabled().unwrap_or(false);
            can_stop = c.IsStopEnabled().unwrap_or(false);
            can_seek = c.IsPlaybackPositionEnabled().unwrap_or(false);
        }

        let (mut position_ms, mut duration_ms) = (None, None);
        if let Ok(tl) = session.GetTimelineProperties() {
            let start = tl.StartTime().map(|t| t.Duration).unwrap_or(0);
            let end = tl.EndTime().map(|t| t.Duration).unwrap_or(0);
            let pos = tl.Position().map(|t| t.Duration).unwrap_or(0);
            let updated = tl.LastUpdatedTime().map(|t| t.UniversalTime).unwrap_or(0);
            let dur = end - start;
            if dur > 0 {
                let mut p = pos - start;
                if status == PlaybackStatus::Playing && updated > 0 {
                    let elapsed = now_ticks() - updated;
                    // Ignore nonsense (clock skew, stale data older than a day).
                    if elapsed > 0 && elapsed < 864_000_000_000 {
                        p += (elapsed as f64 * rate) as i64;
                    }
                }
                let p = p.clamp(0, dur);
                position_ms = Some(p / TICKS_PER_MS);
                duration_ms = Some(dur / TICKS_PER_MS);
            }
        }

        Ok(Some(MediaState {
            app_id,
            title,
            artist,
            album,
            status,
            position_ms,
            duration_ms,
            rate,
            has_artwork,
            can_play,
            can_pause,
            can_next,
            can_prev,
            can_stop,
            can_seek,
        }))
    }

    pub(super) fn media_state() -> Result<Option<MediaState>> {
        worker().run(|| with_manager(read_state))?
    }

    fn thumbnail(mgr: &Manager) -> Option<IRandomAccessStreamReference> {
        let session = current_session(mgr)?;
        let props = session.TryGetMediaPropertiesAsync().ok()?.join().ok()?;
        props.Thumbnail().ok()
    }

    /// Max artwork we are willing to read (thumbnails are normally < 1 MB).
    const MAX_ARTWORK: u64 = 16 * 1024 * 1024;

    fn read_thumbnail(thumb: &IRandomAccessStreamReference) -> Result<Option<Artwork>> {
        let stream = thumb.OpenReadAsync()?.join()?;
        let size = stream.Size()?;
        if size == 0 || size > MAX_ARTWORK {
            return Ok(None);
        }
        let reader = DataReader::CreateDataReader(&stream)?;
        let loaded = reader.LoadAsync(size as u32)?.join()?;
        let mut data = vec![0u8; loaded as usize];
        reader.ReadBytes(&mut data)?;
        let mut mime = stream.ContentType().map(|s| s.to_string()).unwrap_or_default();
        if mime.is_empty() || !mime.starts_with("image/") {
            mime = sniff_mime(&data).to_string();
        }
        Ok(Some(Artwork { mime, data }))
    }

    fn sniff_mime(data: &[u8]) -> &'static str {
        if data.starts_with(&[0x89, b'P', b'N', b'G']) {
            "image/png"
        } else if data.starts_with(&[0xFF, 0xD8, 0xFF]) {
            "image/jpeg"
        } else if data.starts_with(b"GIF8") {
            "image/gif"
        } else if data.len() > 12 && &data[0..4] == b"RIFF" && &data[8..12] == b"WEBP" {
            "image/webp"
        } else if data.starts_with(b"BM") {
            "image/bmp"
        } else {
            "application/octet-stream"
        }
    }

    pub(super) fn media_artwork() -> Result<Option<Artwork>> {
        worker().run(|| {
            with_manager(|mgr| match thumbnail(mgr) {
                Some(t) => read_thumbnail(&t),
                None => Ok(None),
            })
        })?
    }

    fn transcode_png(thumb: &IRandomAccessStreamReference) -> Result<Vec<u8>> {
        let input = thumb.OpenReadAsync()?.join()?;
        let decoder = BitmapDecoder::CreateAsync(&input)?.join()?;
        let bitmap = decoder.GetSoftwareBitmapAsync()?.join()?;
        let out = InMemoryRandomAccessStream::new()?;
        let encoder = BitmapEncoder::CreateAsync(BitmapEncoder::PngEncoderId()?, &out)?.join()?;
        encoder.SetSoftwareBitmap(&bitmap)?;
        encoder.FlushAsync()?.join()?;
        let size = out.Size()?;
        let reader = DataReader::CreateDataReader(&out.GetInputStreamAt(0)?)?;
        let loaded = reader.LoadAsync(size as u32)?.join()?;
        let mut data = vec![0u8; loaded as usize];
        reader.ReadBytes(&mut data)?;
        Ok(data)
    }

    pub(super) fn media_artwork_png() -> Result<Option<Vec<u8>>> {
        worker().run(|| {
            with_manager(|mgr| {
                let Some(thumb) = thumbnail(mgr) else { return Ok(None) };
                match read_thumbnail(&thumb)? {
                    None => Ok(None),
                    Some(a) if a.mime == "image/png" => Ok(Some(a.data)),
                    Some(_) => transcode_png(&thumb).map(Some),
                }
            })
        })?
    }

    pub(super) fn media_control(action: MediaAction) -> Result<()> {
        worker().run(move || {
            with_manager(|mgr| {
                let s = current_session(mgr).ok_or_else(|| Error::NotFound("no media session".into()))?;
                let op = match action {
                    MediaAction::Play => s.TryPlayAsync()?,
                    MediaAction::Pause => s.TryPauseAsync()?,
                    MediaAction::TogglePlayPause => s.TryTogglePlayPauseAsync()?,
                    MediaAction::Next => s.TrySkipNextAsync()?,
                    MediaAction::Previous => s.TrySkipPreviousAsync()?,
                    MediaAction::Stop => s.TryStopAsync()?,
                    MediaAction::SeekMs(ms) => {
                        if ms < 0 {
                            return Err(Error::InvalidArgument("seek position must be >= 0".into()));
                        }
                        let start = s.GetTimelineProperties().and_then(|t| t.StartTime()).map(|t| t.Duration).unwrap_or(0);
                        s.TryChangePlaybackPositionAsync(start + ms.saturating_mul(TICKS_PER_MS))?
                    }
                };
                if op.join()? { Ok(()) } else { Err(Error::Rejected(format!("{action:?} refused by the media app"))) }
            })
        })?
    }

    // ----- watcher -------------------------------------------------------

    enum Msg {
        SessionChanged,
        Changed,
        Stop,
    }

    pub(crate) struct WatcherInner {
        tx: mpsc::Sender<Msg>,
        thread: Option<JoinHandle<()>>,
    }

    struct SessionSubs {
        session: Session,
        props: i64,
        playback: i64,
        timeline: i64,
    }

    impl SessionSubs {
        fn subscribe(session: Session, tx: &mpsc::Sender<Msg>) -> Option<SessionSubs> {
            let (t1, t2, t3) = (tx.clone(), tx.clone(), tx.clone());
            let props = session
                .MediaPropertiesChanged(&TypedEventHandler::new(move |_, _| {
                    let _ = t1.send(Msg::Changed);
                    Ok(())
                }))
                .ok()?;
            let playback = session
                .PlaybackInfoChanged(&TypedEventHandler::new(move |_, _| {
                    let _ = t2.send(Msg::Changed);
                    Ok(())
                }))
                .ok()?;
            let timeline = session
                .TimelinePropertiesChanged(&TypedEventHandler::new(move |_, _| {
                    let _ = t3.send(Msg::Changed);
                    Ok(())
                }))
                .ok()?;
            Some(SessionSubs { session, props, playback, timeline })
        }

        fn unsubscribe(self) {
            let _ = self.session.RemoveMediaPropertiesChanged(self.props);
            let _ = self.session.RemovePlaybackInfoChanged(self.playback);
            let _ = self.session.RemoveTimelinePropertiesChanged(self.timeline);
        }
    }

    /// Longest we delay an update while events keep arriving.
    const MAX_WAIT: Duration = Duration::from_secs(1);

    impl WatcherInner {
        pub(crate) fn start(callback: Callback, debounce: Duration) -> Result<WatcherInner> {
            let (tx, rx) = mpsc::channel::<Msg>();
            let (ready_tx, ready_rx) = mpsc::sync_channel::<Result<()>>(1);
            let tx_thread = tx.clone();
            let thread = thread::Builder::new()
                .name("helm-media-watch".into())
                .spawn(move || {
                    let _com = ComApartment::enter();
                    let mgr = match Manager::RequestAsync().and_then(|op| op.join()) {
                        Ok(m) => m,
                        Err(e) => {
                            let _ = ready_tx.send(Err(e.into()));
                            return;
                        }
                    };
                    let t = tx_thread.clone();
                    let token = match mgr.CurrentSessionChanged(&TypedEventHandler::new(move |_, _| {
                        let _ = t.send(Msg::SessionChanged);
                        Ok(())
                    })) {
                        Ok(tok) => tok,
                        Err(e) => {
                            let _ = ready_tx.send(Err(e.into()));
                            return;
                        }
                    };
                    let _ = ready_tx.send(Ok(()));
                    run_loop(&mgr, &rx, &tx_thread, &callback, debounce);
                    let _ = mgr.RemoveCurrentSessionChanged(token);
                })
                .map_err(|e| Error::Internal(format!("spawn watcher: {e}")))?;
            match ready_rx.recv() {
                Ok(Ok(())) => Ok(WatcherInner { tx, thread: Some(thread) }),
                Ok(Err(e)) => {
                    let _ = thread.join();
                    Err(e)
                }
                Err(_) => Err(Error::Internal("media watcher thread exited".into())),
            }
        }
    }

    fn run_loop(mgr: &Manager, rx: &mpsc::Receiver<Msg>, tx: &mpsc::Sender<Msg>, callback: &Callback, debounce: Duration) {
        let mut subs = current_session(mgr).and_then(|s| SessionSubs::subscribe(s, tx));
        let mut last: Option<(Instant, Option<MediaState>)> = None;
        let mut pending_since: Option<Instant> = Some(Instant::now());
        let mut deadline: Option<Instant> = Some(Instant::now());
        loop {
            let msg = match deadline {
                Some(d) => match rx.recv_timeout(d.saturating_duration_since(Instant::now())) {
                    Ok(m) => Some(m),
                    Err(mpsc::RecvTimeoutError::Timeout) => None,
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                },
                None => match rx.recv() {
                    Ok(m) => Some(m),
                    Err(_) => break,
                },
            };
            match msg {
                Some(Msg::Stop) => break,
                Some(m) => {
                    if matches!(m, Msg::SessionChanged) {
                        if let Some(s) = subs.take() {
                            s.unsubscribe();
                        }
                        subs = current_session(mgr).and_then(|s| SessionSubs::subscribe(s, tx));
                    }
                    let now = Instant::now();
                    let since = *pending_since.get_or_insert(now);
                    deadline = Some((now + debounce).min(since + MAX_WAIT));
                }
                None => {
                    pending_since = None;
                    deadline = None;
                    let state = read_state(mgr).unwrap_or(None);
                    let now = Instant::now();
                    let emit = match &last {
                        None => true,
                        Some((t, prev)) => significant_change(prev, now.duration_since(*t).as_millis() as i64, &state),
                    };
                    if emit {
                        let s2 = state.clone();
                        let _ = catch_unwind(AssertUnwindSafe(|| callback(s2)));
                        last = Some((now, state));
                    }
                }
            }
        }
        if let Some(s) = subs.take() {
            s.unsubscribe();
        }
    }

    impl Drop for WatcherInner {
        fn drop(&mut self) {
            let _ = self.tx.send(Msg::Stop);
            if let Some(t) = self.thread.take() {
                let _ = t.join();
            }
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use super::{Artwork, Callback, MediaAction, MediaState};
    use crate::{Error, Result};
    use std::time::Duration;

    const U: Error = Error::Unsupported("media requires Windows");

    pub(super) fn media_state() -> Result<Option<MediaState>> {
        Err(U)
    }
    pub(super) fn media_artwork() -> Result<Option<Artwork>> {
        Err(U)
    }
    pub(super) fn media_artwork_png() -> Result<Option<Vec<u8>>> {
        Err(U)
    }
    pub(super) fn media_control(_: MediaAction) -> Result<()> {
        Err(U)
    }
    pub(crate) struct WatcherInner;
    impl WatcherInner {
        pub(crate) fn start(_: Callback, _: Duration) -> Result<WatcherInner> {
            Err(U)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> MediaState {
        MediaState {
            app_id: "Spotify.exe".into(),
            title: "Song".into(),
            artist: "Artist".into(),
            album: "Album".into(),
            status: PlaybackStatus::Playing,
            position_ms: Some(10_000),
            duration_ms: Some(200_000),
            rate: 1.0,
            has_artwork: true,
            can_play: false,
            can_pause: true,
            can_next: true,
            can_prev: true,
            can_stop: false,
            can_seek: true,
        }
    }

    #[test]
    fn serde_shape() {
        let v = serde_json::to_value(sample()).unwrap();
        assert_eq!(v["status"], "playing");
        assert_eq!(v["app_id"], "Spotify.exe");
        assert_eq!(v["position_ms"], 10_000);
        assert_eq!(v["can_prev"], true);
        assert_eq!(serde_json::to_value(MediaAction::TogglePlayPause).unwrap(), "toggle_play_pause");
        assert_eq!(serde_json::to_value(MediaAction::SeekMs(5000)).unwrap(), serde_json::json!({"seek_ms": 5000}));
        let a: MediaAction = serde_json::from_str("\"next\"").unwrap();
        assert_eq!(a, MediaAction::Next);
    }

    #[test]
    fn drift_is_not_significant() {
        let a = sample();
        let mut b = sample();
        b.position_ms = Some(15_000);
        assert!(!significant_change(&Some(a.clone()), 5_000, &Some(b.clone())));
        // A seek is significant.
        b.position_ms = Some(60_000);
        assert!(significant_change(&Some(a.clone()), 5_000, &Some(b.clone())));
        // Pause is significant.
        let mut c = sample();
        c.status = PlaybackStatus::Paused;
        assert!(significant_change(&Some(a.clone()), 0, &Some(c)));
        // Session appears/disappears.
        assert!(significant_change(&None, 0, &Some(a.clone())));
        assert!(significant_change(&Some(a), 0, &None));
        assert!(!significant_change(&None, 0, &None));
    }
}
