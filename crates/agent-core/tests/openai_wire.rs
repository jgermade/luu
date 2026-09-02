//! What `OpenAi::stream` actually puts on the socket, and what it makes of what
//! comes back.
//!
//! Same argument as [`ollama_wire`](ollama_wire.rs): the unit tests beside the
//! backend serialize a `Body` the *test* built, and nothing there proves
//! `stream()` builds that body out of the `CompletionRequest` it was handed.
//! That gap is where the window bug shipped once.
//!
//! Here it guards a second claim as well, and this one is the reason this
//! backend was worth writing carefully: `stream_options.include_usage` has to be
//! on the request or these servers stream no usage at all — and the reply half
//! proves that a stream *without* usage comes back as `None` rather than as
//! zeros.
//!
//! The stub is not a general HTTP server. It serves exactly one request, and it
//! knows what that request will be.

use std::net::SocketAddr;

use agent_core::backend::{
    Backend, Chunk, CompletionRequest, Message, StopReason, Usage, openai::OpenAi,
};
use futures_util::StreamExt;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

/// A stream shaped the way a real one is: a role-only first event, two text
/// deltas, the finish reason on its own event, then usage on an event with no
/// choices at all, then `[DONE]`. The blank lines between events and the
/// heartbeat comment are part of what is being tested.
const SSE: &str = concat!(
    "data: {\"choices\":[{\"delta\":{\"role\":\"assistant\"},\"index\":0}]}\n\n",
    ": ping\n\n",
    "data: {\"choices\":[{\"delta\":{\"content\":\"Hola \"},\"index\":0}]}\n\n",
    "data: {\"choices\":[{\"delta\":{\"content\":\"mundo\"},\"index\":0}]}\n\n",
    "data: {\"choices\":[{\"delta\":{},\"index\":0,\"finish_reason\":\"stop\"}]}\n\n",
    "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":26,\"completion_tokens\":7}}\n\n",
    "data: [DONE]\n\n",
);

/// The same stream from a server that ignores `include_usage` — everything up to
/// the finish reason, and then `[DONE]` with no usage event.
const SSE_WITHOUT_USAGE: &str = concat!(
    "data: {\"choices\":[{\"delta\":{\"content\":\"Hola\"},\"index\":0}]}\n\n",
    "data: {\"choices\":[{\"delta\":{},\"index\":0,\"finish_reason\":\"stop\"}]}\n\n",
    "data: [DONE]\n\n",
);

/// Binds an ephemeral port — a fixed one is a test that fails whenever two jobs
/// share a runner — and answers the first request with `body`, handing back the
/// request line, the headers and the JSON it was sent.
async fn stub(sse: &'static str) -> (SocketAddr, JoinHandle<(String, serde_json::Value)>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("binding");
    let address = listener.local_addr().expect("the bound address");

    let served = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.expect("a connection");
        let mut raw: Vec<u8> = Vec::new();

        let head = loop {
            let mut buf = [0u8; 1024];
            let read = socket.read(&mut buf).await.expect("reading the request");
            assert!(read > 0, "the client closed before sending a request");
            raw.extend_from_slice(&buf[..read]);
            if let Some(at) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
                break at + 4;
            }
        };

        let head_text = String::from_utf8_lossy(&raw[..head]).to_string();
        let length: usize = head_text
            .to_lowercase()
            .split("content-length:")
            .nth(1)
            .and_then(|rest| rest.split("\r\n").next())
            .map(|value| value.trim().parse().expect("a numeric content-length"))
            .expect("a content-length on the request");

        while raw.len() < head + length {
            let mut buf = [0u8; 1024];
            let read = socket.read(&mut buf).await.expect("reading the body");
            assert!(read > 0, "the body ended early");
            raw.extend_from_slice(&buf[..read]);
        }

        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\n\r\n{}",
            sse.len(),
            sse,
        );
        socket
            .write_all(response.as_bytes())
            .await
            .expect("writing the response");
        socket.shutdown().await.expect("closing");

        let body = serde_json::from_slice(&raw[head..head + length]).expect("a JSON request body");
        (head_text, body)
    });

    (address, served)
}

async fn round_trip_with(
    sse: &'static str,
    backend: impl Fn(String) -> OpenAi,
    request: CompletionRequest,
) -> (String, serde_json::Value, Vec<Chunk>) {
    let (address, served) = stub(sse).await;
    let backend = backend(format!("http://{address}/v1"));

    let mut chunks = Vec::new();
    let mut stream = backend.stream(request);
    while let Some(chunk) = stream.next().await {
        chunks.push(chunk.expect("a chunk"));
    }

    let (head, body) = served.await.expect("the stub server");
    (head, body, chunks)
}

async fn round_trip(request: CompletionRequest) -> (String, serde_json::Value, Vec<Chunk>) {
    round_trip_with(SSE, OpenAi::new, request).await
}

fn request() -> CompletionRequest {
    CompletionRequest {
        model: "qwen2.5-coder-7b".into(),
        messages: vec![Message::system("you are luu"), Message::user("hola")],
        context_limit: None,
        temperature: None,
        seed: None,
    }
}

#[tokio::test]
async fn the_messages_arrive_in_order_and_the_stream_is_asked_for() {
    let (head, body, _) = round_trip(request()).await;

    assert!(head.starts_with("POST /v1/chat/completions "), "{head}");
    assert_eq!(body["model"], "qwen2.5-coder-7b");
    assert_eq!(body["stream"], true);
    assert_eq!(body["messages"][0]["role"], "system");
    assert_eq!(body["messages"][0]["content"], "you are luu");
    assert_eq!(body["messages"][1]["role"], "user");
    assert_eq!(body["messages"][1]["content"], "hola");
}

/// The claim this backend rests on: usage has to be *asked for*, every time.
#[tokio::test]
async fn every_request_asks_for_the_usage() {
    let (_, body, _) = round_trip(request()).await;
    assert_eq!(body["stream_options"]["include_usage"], true, "{body}");
}

/// The window is budgeted against and **not sent**, because this API has no
/// field for it. A `max_tokens` here would cap the answer at the size of the
/// whole window, which is a different number in a different field.
#[tokio::test]
async fn the_window_is_not_on_the_request_in_any_field() {
    let (_, body, _) = round_trip(CompletionRequest {
        context_limit: Some(8192),
        ..request()
    })
    .await;

    for key in ["max_tokens", "max_completion_tokens", "num_ctx", "n_ctx"] {
        assert!(body.get(key).is_none(), "{key} is on the request: {body}");
    }
    let serialized = body.to_string();
    assert!(!serialized.contains("8192"), "the window leaked: {body}");
}

#[tokio::test]
async fn pinned_sampling_reaches_the_server_and_unpinned_sampling_does_not() {
    let (_, pinned, _) = round_trip(CompletionRequest {
        temperature: Some(0.0),
        seed: Some(42),
        ..request()
    })
    .await;
    assert_eq!(pinned["temperature"], 0.0);
    assert_eq!(pinned["seed"], 42);

    let (_, unpinned, _) = round_trip(request()).await;
    assert!(unpinned.get("temperature").is_none(), "{unpinned}");
    assert!(unpinned.get("seed").is_none(), "{unpinned}");
}

/// No key configured means no header at all: a local `llama-server` wants none,
/// and an empty bearer would make a key-checking server reject with a message
/// about the wrong thing.
#[tokio::test]
async fn the_api_key_is_a_header_only_when_there_is_one() {
    let (head, _, _) = round_trip(request()).await;
    assert!(!head.to_lowercase().contains("authorization"), "{head}");

    let (head, _, _) = round_trip_with(
        SSE,
        |url| OpenAi::new(url).with_api_key("s3cret"),
        request(),
    )
    .await;
    assert!(head.contains("authorization: Bearer s3cret"), "{head}");
}

/// The reply half: SSE comes back as the chunks the turn loop consumes, and the
/// usage that arrives on its own event after the finish reason is not lost.
#[tokio::test]
async fn the_sse_comes_back_as_text_chunks_and_one_done() {
    let (_, _, chunks) = round_trip(request()).await;

    let text: String = chunks
        .iter()
        .filter_map(|chunk| match chunk {
            Chunk::Text(text) => Some(text.as_str()),
            Chunk::Done { .. } => None,
        })
        .collect();
    assert_eq!(text, "Hola mundo");

    assert_eq!(
        chunks
            .iter()
            .filter(|chunk| matches!(chunk, Chunk::Done { .. }))
            .count(),
        1,
        "the stop reason and the usage arrive on two events and are one Done",
    );
    let Some(Chunk::Done { stop, usage }) = chunks.last() else {
        panic!("the stream ended without a Done: {chunks:?}");
    };
    assert_eq!(*stop, StopReason::Stop);
    assert_eq!(
        *usage,
        Some(Usage {
            prompt_tokens: 26,
            completion_tokens: 7,
        })
    );
}

/// And the whole reason `usage` is an `Option`: a server that ignores
/// `include_usage` reports **nothing**, which is not the same as reporting that
/// the prompt was empty.
#[tokio::test]
async fn a_server_that_reports_no_usage_says_none_rather_than_zero() {
    let (_, _, chunks) = round_trip_with(SSE_WITHOUT_USAGE, OpenAi::new, request()).await;

    let Some(Chunk::Done { stop, usage }) = chunks.last() else {
        panic!("the stream ended without a Done: {chunks:?}");
    };
    assert_eq!(*stop, StopReason::Stop);
    assert_eq!(*usage, None, "zero would read as an empty prompt");
}
