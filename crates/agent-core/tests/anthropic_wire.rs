//! What `Anthropic::stream` actually puts on the socket, and what it makes of
//! what comes back.
//!
//! Same argument as [`openai_wire`](openai_wire.rs): a `Body` a unit test
//! built proves nothing about the one `stream()` builds. Here it guards the
//! claim this backend was written for as well — **a thinking block goes back
//! signed** on the next step of a tool loop, with the server's own `tool_use`
//! id, which is what interleaved thinking needs and what no unit test can see,
//! because the memory that does it lives between two requests.
//!
//! The stub serves the responses it was given, one per connection, in order,
//! and hands back every request it read.

use std::net::SocketAddr;

use agent_core::backend::{
    Backend, Chunk, CompletionRequest, Message, ModelFacts, StopReason, ToolSpec, Usage, WindowIs,
    anthropic::{Anthropic, Effort, Thinking},
};
use futures_util::StreamExt;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

/// A reply that thinks and answers: the thinking block streams its text and
/// then its signature, the usage on `message_start` counts what the cache
/// served apart from the rest, and a `ping` sits in the middle.
const THINKS: &str = concat!(
    "event: message_start\n",
    "data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",\"usage\":{\"input_tokens\":4,\"cache_creation_input_tokens\":10,\"cache_read_input_tokens\":30,\"output_tokens\":1}}}\n\n",
    "event: content_block_start\n",
    "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"thinking\",\"thinking\":\"\",\"signature\":\"\"}}\n\n",
    "event: content_block_delta\n",
    "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"they greet\"}}\n\n",
    "event: content_block_delta\n",
    "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"signature_delta\",\"signature\":\"c2ln\"}}\n\n",
    "event: content_block_stop\n",
    "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
    "event: ping\n",
    "data: {\"type\":\"ping\"}\n\n",
    "event: content_block_start\n",
    "data: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
    "event: content_block_delta\n",
    "data: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hola \"}}\n\n",
    "event: content_block_delta\n",
    "data: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"text_delta\",\"text\":\"mundo\"}}\n\n",
    "event: content_block_stop\n",
    "data: {\"type\":\"content_block_stop\",\"index\":1}\n\n",
    "event: message_delta\n",
    "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":12}}\n\n",
    "event: message_stop\n",
    "data: {\"type\":\"message_stop\"}\n\n",
);

/// A reply that thinks and then calls, the call's input in two pieces.
const CALLS: &str = concat!(
    "data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":50,\"output_tokens\":1}}}\n\n",
    "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"thinking\",\"thinking\":\"\",\"signature\":\"\"}}\n\n",
    "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"read it first\"}}\n\n",
    "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"signature_delta\",\"signature\":\"U0lH\"}}\n\n",
    "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
    "data: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"tool_use\",\"id\":\"toolu_01\",\"name\":\"read_file\",\"input\":{}}}\n\n",
    "data: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"pa\"}}\n\n",
    "data: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"th\\\":\\\"README.md\\\"}\"}}\n\n",
    "data: {\"type\":\"content_block_stop\",\"index\":1}\n\n",
    "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\"},\"usage\":{\"output_tokens\":30}}\n\n",
    "data: {\"type\":\"message_stop\"}\n\n",
);

/// The shortest whole reply: text, a stop, and no usage anywhere.
const PLAIN: &str = concat!(
    "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
    "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"It is luu.\"}}\n\n",
    "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"}}\n\n",
    "data: {\"type\":\"message_stop\"}\n\n",
);

struct Seen {
    head: String,
    body: serde_json::Value,
}

/// Binds an ephemeral port and answers one connection per entry of
/// `replies`, each with its status and body, closing after each.
async fn stub(
    replies: Vec<(&'static str, &'static str, &'static str)>,
) -> (SocketAddr, JoinHandle<Vec<Seen>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("binding");
    let address = listener.local_addr().expect("the bound address");

    let served = tokio::spawn(async move {
        let mut seen = Vec::new();
        for (status, kind, reply) in replies {
            let (mut socket, _) = listener.accept().await.expect("a connection");
            let mut raw: Vec<u8> = Vec::new();
            let head = loop {
                let mut buf = [0u8; 4096];
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
                .unwrap_or(0);
            while raw.len() < head + length {
                let mut buf = [0u8; 4096];
                let read = socket.read(&mut buf).await.expect("reading the body");
                assert!(read > 0, "the body ended early");
                raw.extend_from_slice(&buf[..read]);
            }
            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\n\
                 Connection: close\r\n\r\n{reply}",
                reply.len(),
            );
            socket
                .write_all(response.as_bytes())
                .await
                .expect("writing the response");
            socket.shutdown().await.expect("closing");
            let body = match length {
                0 => serde_json::Value::Null,
                _ => serde_json::from_slice(&raw[head..head + length]).expect("a JSON body"),
            };
            seen.push(Seen {
                head: head_text,
                body,
            });
        }
        seen
    });
    (address, served)
}

const SSE: &str = "text/event-stream";

async fn collect(backend: &Anthropic, request: CompletionRequest) -> Vec<Chunk> {
    let mut chunks = Vec::new();
    let mut stream = backend.stream(request);
    while let Some(chunk) = stream.next().await {
        chunks.push(chunk.expect("a chunk"));
    }
    chunks
}

fn text(chunks: &[Chunk]) -> String {
    chunks
        .iter()
        .filter_map(|chunk| match chunk {
            Chunk::Text(text) => Some(text.as_str()),
            Chunk::Done { .. } => None,
        })
        .collect()
}

fn request(messages: Vec<Message>) -> CompletionRequest {
    CompletionRequest {
        tools: Vec::new(),
        model: "claude-opus-5-5".into(),
        messages,
        context_limit: Some(8192),
        temperature: None,
        seed: Some(42),
        constraint: None,
    }
}

fn read_file() -> ToolSpec {
    ToolSpec {
        name: "read_file".into(),
        description: "Read a file.".into(),
        parameters: serde_json::json!({"type": "object", "properties": {"path": {"type": "string"}}}),
    }
}

#[tokio::test]
async fn the_request_is_the_messages_shape_and_the_key_is_its_own_header() {
    let (address, served) = stub(vec![("200 OK", SSE, THINKS)]).await;
    let backend = Anthropic::new(format!("http://{address}/")).with_api_key("sk-ant-test");
    let chunks = collect(
        &backend,
        request(vec![Message::system("you are luu"), Message::user("hola")]),
    )
    .await;
    let seen = served.await.expect("the stub").remove(0);
    let head = seen.head.to_lowercase();
    let body = seen.body;

    assert!(seen.head.starts_with("POST /v1/messages "), "{}", seen.head);
    assert!(head.contains("x-api-key: sk-ant-test"), "{head}");
    assert!(head.contains("anthropic-version: 2023-06-01"), "{head}");
    // Never a bearer: on Claude that is an OAuth token.
    assert!(!head.contains("authorization"), "{head}");

    assert_eq!(body["model"], "claude-opus-5-5");
    assert_eq!(body["stream"], true);
    // Required, and the backend's own number rather than the window.
    assert_eq!(body["max_tokens"], 16384);
    assert_eq!(
        body["system"],
        serde_json::json!([{"type": "text", "text": "you are luu", "cache_control": {"type": "ephemeral"}}])
    );
    assert_eq!(body["messages"][0]["role"], "user");
    assert_eq!(body["messages"][0]["content"][0]["text"], "hola");
    // Pinned in the request, and not sendable: no seed field on this API.
    assert!(body.get("seed").is_none(), "{body}");
    assert!(
        !body.to_string().contains("8192"),
        "the window leaked: {body}"
    );
    for absent in [
        "tools",
        "tool_choice",
        "temperature",
        "thinking",
        "output_config",
    ] {
        assert!(body.get(absent).is_none(), "{absent} sent unasked: {body}");
    }

    // The thinking comes up as the reasoning everything above already reads,
    // and the prompt is every input count, the cached ones included.
    assert_eq!(text(&chunks), "<think>they greet</think>\n\nHola mundo");
    assert!(matches!(
        chunks.last(),
        Some(Chunk::Done {
            stop: StopReason::Stop,
            usage: Some(Usage {
                prompt_tokens: 44,
                completion_tokens: 12
            })
        })
    ));
}

#[tokio::test]
async fn no_key_means_no_key_header_and_no_usage_means_not_reported() {
    let (address, served) = stub(vec![("200 OK", SSE, PLAIN)]).await;
    let backend = Anthropic::new(format!("http://{address}"));
    let chunks = collect(&backend, request(vec![Message::user("hola")])).await;
    let seen = served.await.expect("the stub").remove(0);
    assert!(
        !seen.head.to_lowercase().contains("x-api-key"),
        "{}",
        seen.head
    );
    assert!(seen.body.get("system").is_none(), "{}", seen.body);
    assert!(matches!(
        chunks.last(),
        Some(Chunk::Done {
            stop: StopReason::Stop,
            usage: None
        })
    ));
}

/// Interleaved thinking, end to end across two requests: the step that called
/// comes back with its thinking block signed and the server's own id, and the
/// result answers that id.
#[tokio::test]
async fn the_next_step_of_a_tool_loop_gets_its_thinking_back_signed() {
    let (address, served) = stub(vec![("200 OK", SSE, CALLS), ("200 OK", SSE, PLAIN)]).await;
    let backend = Anthropic::new(format!("http://{address}"))
        .with_thinking(Some(Thinking::Adaptive))
        .with_effort(Some(Effort::High))
        .with_max_tokens(32000);
    let mut history = vec![
        Message::system("you are luu"),
        Message::user("what is in README.md?"),
    ];
    let native = |messages: Vec<Message>| CompletionRequest {
        tools: vec![read_file()],
        seed: None,
        ..request(messages)
    };

    let first = collect(&backend, native(history.clone())).await;
    let said = text(&first);
    assert_eq!(
        said,
        "<think>read it first</think>\n\n```tool\n{\"name\":\"read_file\",\"arguments\":{\"path\":\"README.md\"}}\n```"
    );
    // What the loop does with a step: the reply as text, then its result.
    history.push(Message::assistant(said));
    history.push(Message::user("[read_file] ok\n# luu"));
    let second = collect(&backend, native(history)).await;
    assert_eq!(text(&second), "It is luu.");

    let seen = served.await.expect("the stub");
    let asked = &seen[0].body;
    assert_eq!(asked["max_tokens"], 32000);
    assert_eq!(
        asked["thinking"],
        serde_json::json!({"type": "adaptive", "display": "summarized"})
    );
    assert_eq!(
        asked["output_config"],
        serde_json::json!({"effort": "high"})
    );
    assert_eq!(
        asked["tool_choice"],
        serde_json::json!({"type": "auto", "disable_parallel_tool_use": true})
    );
    assert_eq!(asked["tools"][0]["name"], "read_file");
    assert_eq!(asked["tools"][0]["input_schema"]["type"], "object");

    let messages = &seen[1].body["messages"];
    assert_eq!(
        messages[1],
        serde_json::json!({"role": "assistant", "content": [
            {"type": "thinking", "thinking": "read it first", "signature": "U0lH"},
            {"type": "tool_use", "id": "toolu_01", "name": "read_file", "input": {"path": "README.md"}},
        ]})
    );
    assert_eq!(
        messages[2]["content"][0],
        serde_json::json!({
            "type": "tool_result",
            "tool_use_id": "toolu_01",
            "content": "[read_file] ok\n# luu",
            "cache_control": {"type": "ephemeral"},
        })
    );
}

/// The thinking a budget asks for, and the beta header Claude 4.5 needs to
/// think between calls under it.
#[tokio::test]
async fn a_budget_is_enabled_thinking_and_asks_for_interleaving() {
    let (address, served) = stub(vec![("200 OK", SSE, PLAIN)]).await;
    let backend =
        Anthropic::new(format!("http://{address}")).with_thinking(Some(Thinking::Budget(4096)));
    collect(
        &backend,
        CompletionRequest {
            tools: vec![read_file()],
            ..request(vec![Message::user("hola")])
        },
    )
    .await;
    let seen = served.await.expect("the stub").remove(0);
    assert_eq!(
        seen.body["thinking"],
        serde_json::json!({"type": "enabled", "budget_tokens": 4096})
    );
    assert!(
        seen.head
            .to_lowercase()
            .contains("anthropic-beta: interleaved-thinking-2025-05-14"),
        "{}",
        seen.head
    );
}

#[tokio::test]
async fn a_refusal_before_the_stream_is_the_servers_own_words() {
    let (address, _served) = stub(vec![(
        "401 Unauthorized",
        "application/json",
        "{\"type\":\"error\",\"error\":{\"type\":\"authentication_error\",\"message\":\"invalid x-api-key\"}}",
    )])
    .await;
    let backend = Anthropic::new(format!("http://{address}"));
    let mut stream = backend.stream(request(vec![Message::user("hola")]));
    let error = stream
        .next()
        .await
        .expect("an item")
        .expect_err("a refusal");
    assert!(error.to_string().contains("invalid x-api-key"), "{error}");
}

/// Claude's listing, read for what a person connecting a provider is shown:
/// the ids, the most each model accepts, and a model that takes tools.
#[tokio::test]
async fn the_listing_says_each_models_limit() {
    const MODELS: &str = "{\"data\":[\
        {\"id\":\"claude-opus-5-5\",\"type\":\"model\",\"max_input_tokens\":1000000,\"max_tokens\":128000,\"capabilities\":{\"thinking\":{\"supported\":true}}},\
        {\"id\":\"claude-haiku-4-5\",\"type\":\"model\",\"max_input_tokens\":200000}\
    ],\"has_more\":false}";
    let (address, served) = stub(vec![
        ("200 OK", "application/json", MODELS),
        ("200 OK", "application/json", MODELS),
    ])
    .await;
    let backend = Anthropic::new(format!("http://{address}")).with_api_key("k");
    let models = backend.models().await.expect("a listing");
    assert_eq!(models, ["claude-opus-5-5", "claude-haiku-4-5"]);
    let facts = backend.facts(&models).await.expect("facts");
    assert_eq!(
        facts["claude-opus-5-5"],
        ModelFacts {
            window: Some(1_000_000),
            window_is: Some(WindowIs::Limit),
            tools: Some(true),
        }
    );
    assert_eq!(facts["claude-haiku-4-5"].tools, None);

    let seen = served.await.expect("the stub");
    assert!(
        seen[0].head.starts_with("GET /v1/models?limit=1000 "),
        "{}",
        seen[0].head
    );
    assert!(seen[0].head.to_lowercase().contains("x-api-key: k"));
}

/// A server that does not list is not a failure: nothing to choose from.
#[tokio::test]
async fn a_server_with_no_listing_lists_nothing() {
    let (address, _served) = stub(vec![("404 Not Found", "application/json", "{}")]).await;
    let backend = Anthropic::new(format!("http://{address}"));
    assert_eq!(
        backend.models().await.expect("not an error"),
        Vec::<String>::new()
    );
}
