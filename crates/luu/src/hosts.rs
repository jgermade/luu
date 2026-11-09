//! What a page's request to another luu needs on its way through this one —
//! the headers that stay on this hop, this server's token kept out of a
//! forwarded query, and a socket carried both ways — shared by the links
//! (`crate::link`) and the infer pass-through.
//!
//! It was `[host.<name>]`'s module: another `luu serve` reached by URL and a
//! token in a file. That table was removed on 2026-11-09, once a link by key
//! did everything it did; see
//! `RECORD/2026-11-09.five-decisions.WIP.md` §72(c). What is left here is what
//! both ever shared.

use axum::extract::ws::{CloseFrame, Message as WsMessage, WebSocket};
use axum::http::header;
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite;

/// Request headers that are about *this* hop, or this server's own
/// credential, and never forwarded.
pub(crate) const NOT_FORWARDED: &[header::HeaderName] = &[
    header::HOST,
    header::AUTHORIZATION,
    header::CONNECTION,
    header::TRANSFER_ENCODING,
    header::CONTENT_LENGTH,
    header::UPGRADE,
    header::ORIGIN,
    header::REFERER,
    header::COOKIE,
];

/// Response headers that are about the other hop.
pub(crate) const NOT_RETURNED: &[header::HeaderName] = &[
    header::CONNECTION,
    header::TRANSFER_ENCODING,
    header::CONTENT_LENGTH,
];

/// `rest` with this server's `token` taken out of its query, if it had one.
pub(crate) fn without_token(rest: &str) -> String {
    let Some((path, query)) = rest.split_once('?') else {
        return rest.to_string();
    };
    let kept: Vec<&str> = query
        .split('&')
        .filter(|pair| !pair.is_empty() && pair.split('=').next() != Some("token"))
        .collect();
    match kept.is_empty() {
        true => path.to_string(),
        false => format!("{path}?{}", kept.join("&")),
    }
}

/// Carries frames both ways until either end closes: the page's socket and a
/// linked luu's, over a channel of its link.
pub async fn pipe_to<S>(socket: WebSocket, upstream: tokio_tungstenite::WebSocketStream<S>)
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let (mut to_page, mut from_page) = socket.split();
    let (mut to_host, mut from_host) = upstream.split();
    let up = async {
        while let Some(Ok(message)) = from_page.next().await {
            let message = match message {
                WsMessage::Text(text) => tungstenite::Message::text(text.as_str()),
                WsMessage::Binary(bytes) => tungstenite::Message::binary(bytes),
                WsMessage::Ping(bytes) => tungstenite::Message::Ping(bytes),
                WsMessage::Pong(bytes) => tungstenite::Message::Pong(bytes),
                WsMessage::Close(_) => break,
            };
            if to_host.send(message).await.is_err() {
                break;
            }
        }
        let _ = to_host.close().await;
    };
    let down = async {
        while let Some(Ok(message)) = from_host.next().await {
            let message = match message {
                tungstenite::Message::Text(text) => WsMessage::Text(text.as_str().into()),
                tungstenite::Message::Binary(bytes) => WsMessage::Binary(bytes),
                tungstenite::Message::Ping(bytes) => WsMessage::Ping(bytes),
                tungstenite::Message::Pong(bytes) => WsMessage::Pong(bytes),
                tungstenite::Message::Close(frame) => {
                    let _ = to_page
                        .send(WsMessage::Close(frame.map(|frame| CloseFrame {
                            code: frame.code.into(),
                            reason: frame.reason.as_str().into(),
                        })))
                        .await;
                    break;
                }
                tungstenite::Message::Frame(_) => continue,
            };
            if to_page.send(message).await.is_err() {
                break;
            }
        }
    };
    // Either side ending ends both: a page that left has nobody to read the
    // host, and a host that left has nothing to say.
    tokio::select! {
        _ = up => {}
        _ = down => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn this_servers_token_does_not_travel() {
        assert_eq!(without_token("/ws?token=abc"), "/ws");
        assert_eq!(
            without_token("/api/workspace/tree?path=a&token=abc"),
            "/api/workspace/tree?path=a"
        );
        assert_eq!(
            without_token("/api/workspace/tree?path=a"),
            "/api/workspace/tree?path=a"
        );
        assert_eq!(without_token("/api/settings"), "/api/settings");
    }
}
