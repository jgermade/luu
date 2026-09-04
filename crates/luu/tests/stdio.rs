//! Integration tests for `luu stdio` protocol stream.
//!
//! Asserts that the NDJSON line-oriented protocol over stdin/stdout behaves
//! identically to the WebSocket protocol: Hello greeting, task proposal,
//! approval, turn execution, refusal on busy, and graceful EOF shutdown.

use std::sync::Arc;
use std::time::Duration;

use agent_core::backend::mock::Mock;
use agent_core::context::{ApproximateCounter, Budget, Eviction};
use agent_core::protocol::{ClientMessage, ServerMessage};
use agent_core::repo_map::Order;
use agent_core::sandbox::{Sandbox, SandboxPolicy};
use agent_core::tools::Tools;
use luu::serve::{StdioOptions, serve_stdio_stream};
use luu::session::Agency;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

const PLAN: &str = "```plan\n{\"objective\":\"explain stdio\",\"steps\":[\"describe it\"],\
                    \"files\":[],\"commands\":[]}\n```";

const ANSWER: &str = "The stdio protocol is line-oriented NDJSON.";

fn options_for(replies: Vec<String>) -> StdioOptions {
    let backend = Arc::new(Mock::replies(replies).delay(Duration::ZERO));
    let base = std::env::current_dir().expect("current dir");
    let sandbox = Arc::new(Sandbox::new(&SandboxPolicy::default(), &base).expect("open sandbox"));
    let agency = Agency {
        tools: Arc::new(Tools::standard()),
        sandbox,
        max_steps: 4,
        worker: None,
    };
    let counter = Arc::new(ApproximateCounter);
    let budget = Budget::new(0, 512, Eviction::Turn);
    StdioOptions {
        backend,
        model: "mock".to_string(),
        record: None,
        budget,
        counter,
        agency,
        temperature: None,
        seed: None,
        map_tokens: 0,
        map_order: Order::Path,
        store: None,
    }
}

#[tokio::test]
async fn stdio_greets_with_hello_and_answers_prompts() {
    let options = options_for(vec![PLAN.to_string(), ANSWER.to_string()]);

    // Set up duplex streams for client <-> server communication.
    // client writes to client_out (server reads from server_in).
    // server writes to server_out (client reads from client_in).
    let (server_in, mut client_out) = tokio::io::duplex(4096);
    let (client_in, server_out) = tokio::io::duplex(4096);

    let server_handle = tokio::spawn(async move {
        let reader = BufReader::new(server_in);
        serve_stdio_stream(options, reader, server_out).await
    });

    let mut client_lines = BufReader::new(client_in).lines();

    // 1. Read Hello message.
    let hello_line = client_lines
        .next_line()
        .await
        .expect("read hello")
        .expect("hello line exists");
    let hello: ServerMessage = serde_json::from_str(&hello_line).expect("parse hello");
    match hello {
        ServerMessage::Hello {
            protocol, backend, ..
        } => {
            assert_eq!(protocol, agent_core::protocol::VERSION);
            assert_eq!(backend, "mock");
        }
        other => panic!("expected Hello, got {other:?}"),
    }

    // 2. Send prompt.
    let prompt_msg = serde_json::to_string(&ClientMessage::Prompt {
        text: "how does stdio work?".to_string(),
    })
    .unwrap();
    client_out
        .write_all(format!("{prompt_msg}\n").as_bytes())
        .await
        .unwrap();
    client_out.flush().await.unwrap();

    // Expect TurnStarted, Token(s), Ended, and TaskProposed.
    let mut task_id = None;
    while let Some(line) = client_lines.next_line().await.unwrap() {
        let msg: ServerMessage = serde_json::from_str(&line).expect("parse server message");
        if let ServerMessage::TaskProposed { task, .. } = msg {
            task_id = Some(task);
            break;
        }
    }
    let task = task_id.expect("received task proposal");

    // 3. Approve task.
    let approve_msg = serde_json::to_string(&ClientMessage::ApproveTask {
        task,
        files: vec![],
        writes: vec![],
        commands: vec![],
        closes_on: None,
        network: None,
    })
    .unwrap();
    client_out
        .write_all(format!("{approve_msg}\n").as_bytes())
        .await
        .unwrap();
    client_out.flush().await.unwrap();

    // Expect TaskApproved, TurnStarted, tokens, Ended.
    let mut ended = false;
    while let Some(line) = client_lines.next_line().await.unwrap() {
        let msg: ServerMessage = serde_json::from_str(&line).expect("parse server message");
        if matches!(msg, ServerMessage::Ended { .. }) {
            ended = true;
            break;
        }
    }
    assert!(ended, "turn ended after task approval");

    // 4. Dropping client_out sends EOF on server stdin, which causes graceful shutdown.
    drop(client_out);
    let result = server_handle.await.expect("server join");
    assert!(result.is_ok(), "server terminated cleanly on EOF");
}

#[tokio::test]
async fn unparseable_lines_do_not_break_stdio_stream() {
    let options = options_for(vec![PLAN.to_string(), ANSWER.to_string()]);
    let (server_in, mut client_out) = tokio::io::duplex(4096);
    let (client_in, server_out) = tokio::io::duplex(4096);

    let server_handle = tokio::spawn(async move {
        let reader = BufReader::new(server_in);
        serve_stdio_stream(options, reader, server_out).await
    });

    let mut client_lines = BufReader::new(client_in).lines();

    // Read Hello.
    let _ = client_lines.next_line().await.unwrap().unwrap();

    // Send garbage lines followed by a valid prompt.
    client_out.write_all(b"not valid json\n\n\n").await.unwrap();
    let prompt_msg = serde_json::to_string(&ClientMessage::Prompt {
        text: "hello after garbage".to_string(),
    })
    .unwrap();
    client_out
        .write_all(format!("{prompt_msg}\n").as_bytes())
        .await
        .unwrap();
    client_out.flush().await.unwrap();

    // Should still receive TaskProposed!
    let mut proposed = false;
    while let Some(line) = client_lines.next_line().await.unwrap() {
        let msg: ServerMessage = serde_json::from_str(&line).expect("parse server message");
        if matches!(msg, ServerMessage::TaskProposed { .. }) {
            proposed = true;
            break;
        }
    }
    assert!(proposed, "task proposed despite preceding invalid lines");

    drop(client_out);
    let _ = server_handle.await.unwrap();
}

#[tokio::test]
async fn prompt_while_task_is_pending_is_refused() {
    let options = options_for(vec![PLAN.to_string(), ANSWER.to_string()]);
    let (server_in, mut client_out) = tokio::io::duplex(4096);
    let (client_in, server_out) = tokio::io::duplex(4096);

    let server_handle = tokio::spawn(async move {
        let reader = BufReader::new(server_in);
        serve_stdio_stream(options, reader, server_out).await
    });

    let mut client_lines = BufReader::new(client_in).lines();

    // Read Hello.
    let _ = client_lines.next_line().await.unwrap().unwrap();

    // Send first prompt -> leads to TaskProposed.
    let prompt1 = serde_json::to_string(&ClientMessage::Prompt {
        text: "first prompt".to_string(),
    })
    .unwrap();
    client_out
        .write_all(format!("{prompt1}\n").as_bytes())
        .await
        .unwrap();
    client_out.flush().await.unwrap();

    while let Some(line) = client_lines.next_line().await.unwrap() {
        let msg: ServerMessage = serde_json::from_str(&line).expect("parse server message");
        if matches!(msg, ServerMessage::TaskProposed { .. }) {
            break;
        }
    }

    // Send second prompt while first task is still pending -> must be Refused!
    let prompt2 = serde_json::to_string(&ClientMessage::Prompt {
        text: "second prompt while pending".to_string(),
    })
    .unwrap();
    client_out
        .write_all(format!("{prompt2}\n").as_bytes())
        .await
        .unwrap();
    client_out.flush().await.unwrap();

    let mut refused = false;
    while let Some(line) = client_lines.next_line().await.unwrap() {
        let msg: ServerMessage = serde_json::from_str(&line).expect("parse server message");
        if let ServerMessage::Refused { reason, .. } = msg {
            assert_eq!(reason, agent_core::protocol::Refusal::Pending);
            refused = true;
            break;
        }
    }
    assert!(refused, "second prompt was refused as pending");

    drop(client_out);
    let _ = server_handle.await.unwrap();
}

#[tokio::test]
async fn close_and_reopen_task_over_stdio() {
    let options = options_for(vec![PLAN.to_string(), ANSWER.to_string()]);
    let (server_in, mut client_out) = tokio::io::duplex(4096);
    let (client_in, server_out) = tokio::io::duplex(4096);

    let server_handle = tokio::spawn(async move {
        let reader = BufReader::new(server_in);
        serve_stdio_stream(options, reader, server_out).await
    });

    let mut client_lines = BufReader::new(client_in).lines();

    // Read Hello.
    let _ = client_lines.next_line().await.unwrap().unwrap();

    // 1. Send prompt.
    let prompt_msg = serde_json::to_string(&ClientMessage::Prompt {
        text: "task test".to_string(),
    })
    .unwrap();
    client_out
        .write_all(format!("{prompt_msg}\n").as_bytes())
        .await
        .unwrap();
    client_out.flush().await.unwrap();

    let mut task_id = None;
    while let Some(line) = client_lines.next_line().await.unwrap() {
        let msg: ServerMessage = serde_json::from_str(&line).expect("parse server message");
        if let ServerMessage::TaskProposed { task, .. } = msg {
            task_id = Some(task);
            break;
        }
    }
    let task = task_id.expect("task proposed");

    // 2. Approve task.
    let approve_msg = serde_json::to_string(&ClientMessage::ApproveTask {
        task,
        files: vec![],
        writes: vec![],
        commands: vec![],
        closes_on: None,
        network: None,
    })
    .unwrap();
    client_out
        .write_all(format!("{approve_msg}\n").as_bytes())
        .await
        .unwrap();
    client_out.flush().await.unwrap();

    // Wait for turn to end.
    while let Some(line) = client_lines.next_line().await.unwrap() {
        let msg: ServerMessage = serde_json::from_str(&line).expect("parse server message");
        if matches!(msg, ServerMessage::Ended { .. }) {
            break;
        }
    }

    // 3. Close task.
    let close_msg = serde_json::to_string(&ClientMessage::CloseTask { task }).unwrap();
    client_out
        .write_all(format!("{close_msg}\n").as_bytes())
        .await
        .unwrap();
    client_out.flush().await.unwrap();

    let mut closed = false;
    while let Some(line) = client_lines.next_line().await.unwrap() {
        let msg: ServerMessage = serde_json::from_str(&line).expect("parse server message");
        if let ServerMessage::TaskClosed {
            task: closed_task, ..
        } = msg
        {
            assert_eq!(closed_task, task);
            closed = true;
            break;
        }
    }
    assert!(closed, "task was closed");

    // 4. Reopen task.
    let reopen_msg = serde_json::to_string(&ClientMessage::ReopenTask { task }).unwrap();
    client_out
        .write_all(format!("{reopen_msg}\n").as_bytes())
        .await
        .unwrap();
    client_out.flush().await.unwrap();

    let mut reopened = false;
    while let Some(line) = client_lines.next_line().await.unwrap() {
        let msg: ServerMessage = serde_json::from_str(&line).expect("parse server message");
        if let ServerMessage::TaskReopened {
            task: reopened_task,
        } = msg
        {
            assert_eq!(reopened_task, task);
            reopened = true;
            break;
        }
    }
    assert!(reopened, "task was reopened");

    drop(client_out);
    let _ = server_handle.await.unwrap();
}
