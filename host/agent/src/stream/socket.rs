//! The `/stream` WebSocket (PROTOCOL §10): `hello` → `auth` (no `pair`) → `ready` →
//! `{"type":"stream.open","token"}` → binary frames from the agent until the stream stops.

use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::watch;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;

use super::TokenError;
use crate::proto::codes;
use crate::server::{AppState, First, fail, handshake, read_first};

pub async fn run<S>(
    state: Arc<AppState>,
    ws: WebSocketStream<S>,
    peer: String,
    mut shutdown: watch::Receiver<bool>,
) where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let Some((mut ws, device_id, _tier)) = handshake(&state, ws, &peer, false).await else {
        return;
    };
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let text = match read_first(&mut ws, deadline).await {
        First::Text(t) => t,
        First::Binary => return fail(ws, codes::BAD_MESSAGE, "expected stream.open").await,
        First::TimedOut => return fail(ws, codes::TIMEOUT, "no stream.open in time").await,
        First::Gone => return,
    };
    let token = match serde_json::from_str::<Value>(&text) {
        Ok(v) if v.get("type").and_then(Value::as_str) == Some("stream.open") => {
            match v.get("token").and_then(Value::as_str) {
                Some(t) => t.to_string(),
                None => return fail(ws, codes::BAD_MESSAGE, "stream.open needs a token").await,
            }
        }
        _ => return fail(ws, codes::BAD_MESSAGE, "expected stream.open").await,
    };
    let taken = state
        .lock_stream_tokens()
        .take(&token, &device_id, Instant::now());
    let stream = match taken {
        Ok(weak) => match weak.upgrade() {
            Some(s) if !s.is_stopped() => s,
            _ => return fail(ws, codes::BAD_TOKEN, "that stream has already stopped").await,
        },
        Err(TokenError::Expired) => {
            return fail(ws, codes::BAD_TOKEN, "stream token expired").await;
        }
        Err(TokenError::Invalid) => {
            return fail(ws, codes::BAD_TOKEN, "stream token is wrong or already used").await;
        }
    };
    tracing::info!("stream {} opened by {device_id} from {peer}", stream.id);
    stream.attach();

    let (mut sink, mut incoming) = ws.split();
    let writer_stream = stream.clone();
    let mut writer = tokio::spawn(async move {
        while let Some(frame) = writer_stream.next_frame().await {
            if sink.send(Message::binary(frame)).await.is_err() {
                return;
            }
        }
        let close = CloseFrame {
            code: CloseCode::Normal,
            reason: "stream stopped".into(),
        };
        let _ = sink.send(Message::Close(Some(close))).await;
        let _ = sink.flush().await;
    });

    let mut writer_done = false;
    let mut shutting_down = false;
    loop {
        tokio::select! {
            msg = incoming.next() => match msg {
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                Some(Ok(_)) => {} // nothing else is expected from the phone here
            },
            _ = &mut writer, if !writer_done => {
                writer_done = true;
                let _ = tokio::time::timeout(Duration::from_secs(2), async {
                    while let Some(Ok(_)) = incoming.next().await {}
                }).await;
                break;
            }
            _ = shutdown.changed(), if !shutting_down => {
                shutting_down = true;
                stream.stop();
            }
        }
    }
    // The viewer is gone: stop capturing (the phone starts a new stream to resume).
    stream.stop();
    if let Some(session) = state
        .sessions()
        .into_iter()
        .find(|s| s.stream().is_some_and(|a| a.id == stream.id))
    {
        session.clear_stream(stream.id);
    }
    if !writer_done && tokio::time::timeout(Duration::from_secs(2), &mut writer).await.is_err() {
        writer.abort(); // stuck on a socket nobody reads
    }
    tracing::debug!("stream {} socket closed", stream.id);
}
