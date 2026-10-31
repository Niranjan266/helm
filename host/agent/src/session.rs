//! One ready connection: its outgoing queue, input injector, uploads and per-session flags.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc as std_mpsc};

use serde_json::{Value, json};
use tokio::sync::{OwnedSemaphorePermit, mpsc};
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;

use crate::files::{TransferId, Upload};
use crate::platform::{InputEvent, Platform, Rect};
use crate::stream::ActiveStream;
use crate::proto;
use crate::router::Tier;

pub(crate) enum Outgoing {
    Text(String),
    /// Binary frame; the permit (if any) is released once the frame was written.
    Binary(Vec<u8>, Option<OwnedSemaphorePermit>),
    Close(CloseCode, &'static str),
}

/// `gy{dx,dy}` (gyro air mouse, PROTOCOL §12) is handled exactly like `mm`.
fn gyro_as_move(mut frame: Value) -> Value {
    if frame.get("k").and_then(Value::as_str) == Some("gy") {
        frame["k"] = Value::from("mm");
    }
    frame
}

pub type AfterResponse = Pin<Box<dyn Future<Output = ()> + Send>>;

type SharedUpload = Arc<tokio::sync::Mutex<Option<Upload>>>;

/// What the per-session input thread does.
enum InputCmd {
    Event(InputEvent),
    /// `ma`: absolute move to a virtual-desktop pixel.
    MoveTo(i32, i32),
}

pub struct Session {
    pub id: u64,
    pub device_id: String,
    pub tier: Tier,
    tx: mpsc::UnboundedSender<Outgoing>,
    clipboard_watch: AtomicBool,
    input: Mutex<Option<std_mpsc::Sender<InputCmd>>>,
    /// Last input error code reported to the phone (reported once until input works again).
    input_error: Arc<Mutex<Option<String>>>,
    uploads: Mutex<HashMap<TransferId, SharedUpload>>,
    after: Mutex<HashMap<i64, AfterResponse>>,
    /// The screen stream started by this session (`stream.start`), if any.
    stream: Mutex<Option<Arc<ActiveStream>>>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

impl Session {
    pub(crate) fn new(
        id: u64,
        device_id: String,
        tier: Tier,
        tx: mpsc::UnboundedSender<Outgoing>,
    ) -> Session {
        Session {
            id,
            device_id,
            tier,
            tx,
            clipboard_watch: AtomicBool::new(false),
            input: Mutex::new(None),
            input_error: Arc::new(Mutex::new(None)),
            uploads: Mutex::new(HashMap::new()),
            after: Mutex::new(HashMap::new()),
            stream: Mutex::new(None),
        }
    }

    /// A session whose messages go nowhere (unit tests).
    pub fn detached(id: u64, device_id: &str, tier: Tier) -> Session {
        let (tx, _rx) = mpsc::unbounded_channel();
        Session::new(id, device_id.to_string(), tier, tx)
    }

    pub(crate) fn send(&self, out: Outgoing) -> bool {
        self.tx.send(out).is_ok()
    }

    pub fn send_text(&self, text: String) -> bool {
        self.send(Outgoing::Text(text))
    }

    pub fn send_event(&self, name: &str, data: Value) -> bool {
        self.send_text(proto::event(name, data).to_string())
    }

    pub fn send_binary(&self, frame: Vec<u8>, permit: OwnedSemaphorePermit) -> bool {
        self.send(Outgoing::Binary(frame, Some(permit)))
    }

    pub fn close(&self, code: CloseCode, reason: &'static str) {
        let _ = self.tx.send(Outgoing::Close(code, reason));
    }

    pub fn clipboard_watch(&self) -> bool {
        self.clipboard_watch.load(Ordering::SeqCst)
    }

    pub fn set_clipboard_watch(&self, on: bool) {
        self.clipboard_watch.store(on, Ordering::SeqCst);
    }

    /// Run `work` right after the response to request `req_id` was queued.
    pub fn after_response(&self, req_id: i64, work: AfterResponse) {
        lock(&self.after).insert(req_id, work);
    }

    pub(crate) fn take_after(&self, req_id: i64) -> Option<AfterResponse> {
        lock(&self.after).remove(&req_id)
    }

    // ---- stream ----

    pub fn stream(&self) -> Option<Arc<ActiveStream>> {
        lock(&self.stream).clone()
    }

    /// Install a new stream; returns the one it replaces (the caller stops it).
    pub fn set_stream(&self, stream: Arc<ActiveStream>) -> Option<Arc<ActiveStream>> {
        lock(&self.stream).replace(stream)
    }

    pub fn take_stream(&self) -> Option<Arc<ActiveStream>> {
        lock(&self.stream).take()
    }

    /// Forget stream `id` if it is still the current one.
    pub fn clear_stream(&self, id: u64) {
        let mut s = lock(&self.stream);
        if s.as_ref().is_some_and(|a| a.id == id) {
            *s = None;
        }
    }

    // ---- input ----

    fn report_input_error(
        tx: &mpsc::UnboundedSender<Outgoing>,
        last: &Mutex<Option<String>>,
        code: &str,
    ) {
        let mut last = lock(last);
        if last.as_deref() != Some(code) {
            *last = Some(code.to_string());
            let ev = proto::event("input.error", json!({"code": code}));
            let _ = tx.send(Outgoing::Text(ev.to_string()));
        }
    }

    /// Handle one `in` frame (already parsed as JSON). Never replies, except `input.error`.
    pub fn handle_input(&self, platform: &Arc<dyn Platform>, frame: Value) {
        if self.tier < Tier::Remote {
            return Self::report_input_error(&self.tx, &self.input_error, "forbidden");
        }
        let cmd = if frame.get("k").and_then(Value::as_str) == Some("ma") {
            let (Some(x), Some(y)) = (
                frame.get("x").and_then(Value::as_f64),
                frame.get("y").and_then(Value::as_f64),
            ) else {
                return Self::report_input_error(&self.tx, &self.input_error, "bad_params");
            };
            let Some(rect) = self.pointer_rect(platform) else {
                return Self::report_input_error(&self.tx, &self.input_error, "internal");
            };
            let (px, py) = crate::stream::normalized_to_desktop(x, y, rect);
            InputCmd::MoveTo(px, py)
        } else {
            match serde_json::from_value(gyro_as_move(frame)) {
                Ok(e) => InputCmd::Event(e),
                Err(_) => {
                    return Self::report_input_error(&self.tx, &self.input_error, "bad_params");
                }
            }
        };
        let mut input = lock(&self.input);
        if input.is_none() {
            match platform.new_injector() {
                Ok(injector) => *input = Some(self.spawn_input_thread(injector)),
                Err(e) => {
                    tracing::warn!("input unavailable: {}", e.message);
                    return Self::report_input_error(&self.tx, &self.input_error, "internal");
                }
            }
        }
        if let Some(sender) = input.as_ref()
            && sender.send(cmd).is_err()
        {
            *input = None;
        }
    }

    /// One thread per session injects events in order; it releases everything held when
    /// the session ends (the sender is dropped).
    fn spawn_input_thread(
        &self,
        injector: crate::platform::Injector,
    ) -> std_mpsc::Sender<InputCmd> {
        let (sender, receiver) = std_mpsc::channel::<InputCmd>();
        let tx = self.tx.clone();
        let last_error = self.input_error.clone();
        let session_id = self.id;
        let spawned = std::thread::Builder::new()
            .name(format!("helm-input-{session_id}"))
            .spawn(move || {
                while let Ok(cmd) = receiver.recv() {
                    let result = match cmd {
                        InputCmd::Event(event) => injector.inject(&event),
                        InputCmd::MoveTo(x, y) => injector.move_to(x, y),
                    };
                    match result {
                        Ok(()) => *lock(&last_error) = None,
                        Err(helm_win_input::Error::BlockedByElevation) => {
                            Self::report_input_error(&tx, &last_error, "blocked_by_elevation")
                        }
                        Err(e) => {
                            tracing::debug!("input failed: {e}");
                            Self::report_input_error(&tx, &last_error, "input_failed")
                        }
                    }
                }
                if let Err(e) = injector.release_all() {
                    tracing::debug!("release_all failed: {e}");
                }
            });
        if let Err(e) = spawned {
            tracing::warn!("could not start input thread: {e}");
        }
        sender
    }

    /// `ma` maps onto the streamed source, or the primary monitor when nothing streams.
    fn pointer_rect(&self, platform: &Arc<dyn Platform>) -> Option<Rect> {
        if let Some(rect) = self.stream().and_then(|s| s.source_rect()) {
            return Some(rect);
        }
        let monitors = platform.monitors().ok()?;
        monitors
            .iter()
            .find(|m| m.is_primary)
            .or(monitors.first())
            .map(Rect::from)
    }

    // ---- uploads ----

    pub fn upload_count(&self) -> usize {
        lock(&self.uploads).len()
    }

    pub fn add_upload(&self, id: TransferId, upload: Upload) {
        lock(&self.uploads).insert(id, Arc::new(tokio::sync::Mutex::new(Some(upload))));
    }

    fn get_upload(&self, id: &TransferId) -> Option<SharedUpload> {
        lock(&self.uploads).get(id).cloned()
    }

    /// Remove an upload, waiting for any write in progress.
    pub async fn take_upload(&self, id: &TransferId) -> Option<Upload> {
        let shared = lock(&self.uploads).remove(id)?;
        shared.lock().await.take()
    }

    /// Apply one binary frame. Unknown transfer ids are ignored.
    pub async fn write_upload(&self, id: &TransferId, offset: u64, data: &[u8]) {
        let Some(shared) = self.get_upload(id) else {
            tracing::debug!("binary frame for unknown transfer {}", hex::encode(id));
            return;
        };
        if let Some(upload) = shared.lock().await.as_mut() {
            upload.write(offset, data).await;
        }
    }

    /// Session ended: stop input (releases held keys/buttons) and drop partial uploads.
    pub(crate) async fn finish(&self) {
        if let Some(s) = self.take_stream() {
            s.stop();
        }
        drop(lock(&self.input).take());
        lock(&self.after).clear();
        let uploads: Vec<SharedUpload> = lock(&self.uploads).drain().map(|(_, u)| u).collect();
        for u in uploads {
            if let Some(upload) = u.lock().await.take() {
                upload.discard();
            }
        }
    }
}
