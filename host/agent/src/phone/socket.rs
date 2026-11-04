//! The phone's half of `/stream` (g9): `{"type":"phone.open","token"}` after the normal handshake,
//! then binary frames phone → PC and `phone.in` events PC → phone until either side stops.

use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::watch;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;

use super::{Assembled, Reassembler};
use crate::proto::codes;
use crate::server::{AppState, fail};
use crate::stream::TokenError;

pub async fn run<S>(
    state: Arc<AppState>,
    ws: WebSocketStream<S>,
    device_id: String,
    token: String,
    mut shutdown: watch::Receiver<bool>,
) where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (id, mut outgoing) = match state.phone.open(&state, &token, &device_id) {
        Ok(ok) => ok,
        Err(TokenError::Expired) => return fail(ws, codes::BAD_TOKEN, "phone token expired").await,
        Err(TokenError::Invalid) => {
            return fail(ws, codes::BAD_TOKEN, "phone token is wrong or already used").await;
        }
    };
    tracing::info!("phone session {id} opened by {device_id}");
    let (mut sink, mut incoming) = ws.split();
    let mut assembler = Reassembler::default();
    let mut open = true;
    let mut shutting_down = false;
    while open {
        tokio::select! {
            msg = incoming.next() => match msg {
                Some(Ok(Message::Binary(b))) => match assembler.push(b.to_vec()) {
                    Assembled::Frame { frame, key } => state.phone.on_frame(id, frame, key),
                    Assembled::Lost => state.phone.request_keyframe(),
                    Assembled::Pending => {}
                },
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => open = false,
                Some(Ok(_)) => {} // the phone says everything else on the control socket
            },
            text = outgoing.recv() => match text {
                Some(text) => {
                    if sink.send(Message::text(text)).await.is_err() {
                        open = false;
                    }
                }
                // The session ended on the PC side: nothing more to carry.
                None => open = false,
            },
            _ = shutdown.changed(), if !shutting_down => {
                shutting_down = true;
                state.phone.end_for_shutdown(&state, id);
            }
        }
    }
    state.phone.on_stream_closed(&state, id);
    let close = CloseFrame {
        code: CloseCode::Normal,
        reason: "phone session ended".into(),
    };
    let _ = tokio::time::timeout(Duration::from_secs(2), async {
        let _ = sink.send(Message::Close(Some(close))).await;
        let _ = sink.flush().await;
    })
    .await;
    tracing::debug!("phone session {id} socket closed");
}
