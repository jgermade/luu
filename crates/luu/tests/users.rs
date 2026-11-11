//! A public `luu serve`: users who log in with a code, and who never reach
//! each other's sessions or folders. See
//! `RECORD/2026-11-03.a-user-a-code-and-a-folder.completed.md`.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use agent_core::backend::mock::Mock;
use agent_core::context::{ApproximateCounter, Budget, Eviction};
use agent_core::sandbox::{Access, Sandbox, SandboxPolicy};
use agent_core::tools::Tools;
use luu::serve::{ServeOptions, bind};
use luu::session::Agency;
use luu::users::{Role, Users};
use serde_json::Value;

const ORIGIN: &str = "http://luu.test";

/// A scratch directory: `work/` is `serve`'s root, beside `auth.db` and
/// `sessions.db`.
fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("luu-users-it-{name}-{}", std::process::id()));
    std::fs::remove_dir_all(&dir).ok();
    std::fs::create_dir_all(dir.join("work")).expect("a scratch directory");
    dir
}

/// What every session's policy grants: `grant`, read-write, relative to the
/// session's folder — `.` is the folder itself.
fn policy(grant: &Path) -> SandboxPolicy {
    let mut policy = SandboxPolicy::default();
    policy.paths.push(agent_core::sandbox::PathRule::new(
        grant.to_str().expect("a path"),
        Access::ReadWrite,
    ));
    policy
}

fn agency(policy: &SandboxPolicy, base: &Path) -> Agency {
    Agency::new(
        Arc::new(Tools::standard()),
        Arc::new(Sandbox::new(policy, base).expect("the sandbox")),
        agent_core::agent::Limits::default().with_max_steps(4),
        None,
    )
}

/// A public server over `dir/work`, its sessions made under `grant`.
async fn public_server(dir: &Path, grant: &Path) -> String {
    let root = dir.join("work").canonicalize().expect("the root");
    let granted = policy(grant);
    let factory_policy = granted.clone();
    let factory_root = root.clone();
    let serving = bind(ServeOptions {
        icons: Arc::new(luu::icons::Theme::default()),
        provider: luu::provider::Resolved {
            named: true,
            ..luu::provider::Resolved::mock()
        },
        counter_warning: None,
        approvers: Default::default(),
        max_tool_steps: Some(4),
        address: "127.0.0.1:0".parse().expect("an address"),
        backend: Arc::new(Mock::replies(vec![]).delay(Duration::ZERO)),
        model: "mock".into(),
        record: None,
        budget: Budget::new(0, 0, Eviction::Turn),
        counter: Arc::new(ApproximateCounter),
        tokenizer: None,
        agency: agency(&granted, &root),
        temperature: None,
        seed: None,
        map_tokens: 0,
        map_order: Default::default(),
        map_fill: Default::default(),
        select_tokens: 0,
        select_weights: Default::default(),
        constrain: None,
        auth_token_file: None,
        exposure: Some(luu::provider::Exposure::Public),
        origin: Some(ORIGIN.into()),
        auth_db: Some(dir.join("auth.db")),
        auth: Default::default(),
        store: Some(dir.join("sessions.db")),
        agency_for: Some(Arc::new(move |_chosen, _runtime, folder| {
            let base = folder.unwrap_or_else(|| factory_root.clone());
            let policy = factory_policy.clone();
            Box::pin(async move { Ok(agency(&policy, &base)) })
        })),
        describe_for: None,
    })
    .await
    .expect("binding the server");
    let address = serving.address().to_string();
    tokio::spawn(serving.run());
    address
}

/// Logs `email` in with a code `luu user code` would have printed, and
/// returns the `Cookie` header that carries the login.
async fn log_in(address: &str, users: &Users, email: &str) -> String {
    let code = users.issue_code(email).expect("a code");
    let response = reqwest::Client::new()
        .post(format!("http://{address}/api/auth/login"))
        .header("Origin", ORIGIN)
        .json(&serde_json::json!({ "email": email, "code": code }))
        .send()
        .await
        .expect("the login");
    assert_eq!(response.status(), 204, "{:?}", response.text().await);
    let set = response
        .headers()
        .get("set-cookie")
        .expect("a cookie")
        .to_str()
        .unwrap()
        .to_string();
    assert!(
        set.contains("HttpOnly") && set.contains("SameSite=Strict"),
        "{set}"
    );
    // `http://` origin: no `Secure`, which a browser would drop here.
    assert!(!set.contains("Secure"), "{set}");
    set.split(';').next().unwrap().to_string()
}

async fn get(address: &str, path: &str, cookie: Option<&str>) -> reqwest::Response {
    let mut request = reqwest::Client::new().get(format!("http://{address}{path}"));
    if let Some(cookie) = cookie {
        request = request.header("Cookie", cookie);
    }
    request.send().await.expect("the request")
}

async fn json(address: &str, path: &str, cookie: &str) -> Value {
    let response = get(address, path, Some(cookie)).await;
    assert_eq!(response.status(), 200, "GET {path}");
    response.json().await.expect("JSON")
}

#[tokio::test]
async fn nobody_gets_in_without_a_login() {
    let dir = scratch("nobody");
    let users = Users::open(&dir.join("auth.db")).unwrap();
    users.add("ana@example.com", Role::Owner).unwrap();
    let address = public_server(&dir, Path::new(".")).await;

    assert_eq!(get(&address, "/api/sessions", None).await.status(), 401);
    let me = get(&address, "/api/auth/me", None).await;
    assert_eq!(me.status(), 401);
    assert_eq!(me.json::<Value>().await.unwrap()["users"], true);
    // Not even with a token in the URL: public accepts none.
    assert_eq!(
        get(&address, "/api/sessions?token=anything", None)
            .await
            .status(),
        401
    );
    // A wrong code is the same answer as no user at all.
    let wrong = reqwest::Client::new()
        .post(format!("http://{address}/api/auth/login"))
        .header("Origin", ORIGIN)
        .json(&serde_json::json!({ "email": "ana@example.com", "code": "000000" }))
        .send()
        .await
        .unwrap();
    assert_eq!(wrong.status(), 401);
    let stranger = reqwest::Client::new()
        .post(format!("http://{address}/api/auth/login"))
        .header("Origin", ORIGIN)
        .json(&serde_json::json!({ "email": "eve@example.com", "code": "000000" }))
        .send()
        .await
        .unwrap();
    assert_eq!(stranger.status(), 401);
    assert_eq!(
        wrong.text().await.unwrap(),
        stranger.text().await.unwrap(),
        "a stranger and a wrong code read the same"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn a_login_from_another_site_is_refused() {
    let dir = scratch("origin");
    let users = Users::open(&dir.join("auth.db")).unwrap();
    users.add("ana@example.com", Role::Owner).unwrap();
    let address = public_server(&dir, Path::new(".")).await;

    let code = users.issue_code("ana@example.com").unwrap();
    let response = reqwest::Client::new()
        .post(format!("http://{address}/api/auth/login"))
        .header("Origin", "https://evil.example")
        .json(&serde_json::json!({ "email": "ana@example.com", "code": code }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 403);

    // Logged in properly, a write from another site is still refused.
    let cookie = log_in(&address, &users, "ana@example.com").await;
    let write = reqwest::Client::new()
        .post(format!("http://{address}/api/sessions"))
        .header("Cookie", &cookie)
        .header("Origin", "https://evil.example")
        .json(&serde_json::json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(write.status(), 403);
    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn two_users_never_see_each_other() {
    let dir = scratch("two");
    let users = Users::open(&dir.join("auth.db")).unwrap();
    let ana = users.add("ana@example.com", Role::Owner).unwrap();
    let bo = users.add("bo@example.com", Role::User).unwrap();
    let address = public_server(&dir, Path::new(".")).await;

    let ana_cookie = log_in(&address, &users, "ana@example.com").await;
    let bo_cookie = log_in(&address, &users, "bo@example.com").await;

    let me = json(&address, "/api/auth/me", &bo_cookie).await;
    assert_eq!(me["email"], "bo@example.com");
    assert_eq!(me["role"], "user");

    // Each has a session of their own, made at their first request, in a
    // folder of their own.
    let ana_sessions = json(&address, "/api/sessions", &ana_cookie).await;
    let bo_sessions = json(&address, "/api/sessions", &bo_cookie).await;
    let ids = |list: &Value| -> Vec<String> {
        list.as_array()
            .unwrap()
            .iter()
            .map(|row| row["id"].as_str().unwrap().to_string())
            .collect()
    };
    // Each sees one row: their own current session, which the list calls
    // `live`.
    assert_eq!(ids(&ana_sessions), ["live"], "{ana_sessions}");
    assert_eq!(ids(&bo_sessions), ["live"], "{bo_sessions}");
    // A second session of Ana's puts the first beside it under its own id.
    let made = reqwest::Client::new()
        .post(format!("http://{address}/api/sessions"))
        .header("Cookie", &ana_cookie)
        .header("Origin", ORIGIN)
        .json(&serde_json::json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(made.status(), 201, "{:?}", made.text().await);
    let ana_ids = ids(&json(&address, "/api/sessions", &ana_cookie).await);
    assert_eq!(ana_ids.len(), 2, "{ana_ids:?}");
    // And Bo still sees only his.
    assert_eq!(
        ids(&json(&address, "/api/sessions", &bo_cookie).await),
        ["live"]
    );

    let ana_settings = json(&address, "/api/settings", &ana_cookie).await;
    let bo_settings = json(&address, "/api/settings", &bo_cookie).await;
    assert!(
        ana_settings["base"].as_str().unwrap().ends_with(&ana.id),
        "{}",
        ana_settings["base"]
    );
    assert!(
        bo_settings["base"].as_str().unwrap().ends_with(&bo.id),
        "{}",
        bo_settings["base"]
    );
    assert!(dir.join("work").join(&ana.id).is_dir());

    // Bo cannot name Ana's session, by path or by `?session=`, and is told
    // it is not there rather than that it is not his.
    let ana_id = ana_ids.iter().find(|id| *id != "live").unwrap();
    for path in [
        format!("/api/sessions/{ana_id}"),
        format!("/api/sessions/{ana_id}/turns"),
        format!("/api/settings?session={ana_id}"),
        format!("/api/workspace/tree?path=&session={ana_id}"),
    ] {
        assert_eq!(
            get(&address, &path, Some(&bo_cookie)).await.status(),
            404,
            "{path}"
        );
    }
    let resume = reqwest::Client::new()
        .post(format!("http://{address}/api/sessions/{ana_id}/resume"))
        .header("Cookie", &bo_cookie)
        .header("Origin", ORIGIN)
        .json(&serde_json::json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(resume.status(), 404);

    // Nor her folder: the picker browses his own, and a folder outside it is
    // refused.
    std::fs::create_dir_all(dir.join("work").join(&ana.id).join("secret")).unwrap();
    let folders = json(&address, "/api/folders?path=", &bo_cookie).await;
    assert!(
        !folders.to_string().contains(&ana.id) && !folders.to_string().contains("secret"),
        "{folders}"
    );
    let escape = reqwest::Client::new()
        .post(format!("http://{address}/api/sessions"))
        .header("Cookie", &bo_cookie)
        .header("Origin", ORIGIN)
        .json(&serde_json::json!({ "folder": format!("../{}/secret", ana.id) }))
        .send()
        .await
        .unwrap();
    assert_eq!(escape.status(), 400, "{:?}", escape.text().await);

    // Logging out ends the login.
    let out = reqwest::Client::new()
        .post(format!("http://{address}/api/auth/logout"))
        .header("Cookie", &bo_cookie)
        .header("Origin", ORIGIN)
        .send()
        .await
        .unwrap();
    assert_eq!(out.status(), 204);
    assert_eq!(
        get(&address, "/api/sessions", Some(&bo_cookie))
            .await
            .status(),
        401
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn a_policy_that_reaches_every_folder_is_refused_for_a_user() {
    let dir = scratch("reach");
    let users = Users::open(&dir.join("auth.db")).unwrap();
    users.add("ana@example.com", Role::Owner).unwrap();
    // `/work` itself, as `deploy/luu.toml` granted it before this change.
    let root = dir.join("work").canonicalize().unwrap();
    let address = public_server(&dir, &root).await;

    let cookie = log_in(&address, &users, "ana@example.com").await;
    let response = get(&address, "/api/sessions", Some(&cookie)).await;
    assert_eq!(response.status(), 403);
    let said = response.text().await.unwrap();
    assert!(said.contains("every user's folder"), "{said}");
    std::fs::remove_dir_all(&dir).ok();
}

/// A protocol socket as the page opens it: the login's cookie, and this
/// server's `Origin`.
async fn socket(
    address: &str,
    cookie: &str,
    query: &str,
    origin: &str,
) -> Result<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    tokio_tungstenite::tungstenite::Error,
> {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    let mut request = format!("ws://{address}/ws{query}")
        .into_client_request()
        .unwrap();
    request
        .headers_mut()
        .insert("Cookie", cookie.parse().unwrap());
    request
        .headers_mut()
        .insert("Origin", origin.parse().unwrap());
    tokio_tungstenite::connect_async(request)
        .await
        .map(|(stream, _)| stream)
}

async fn next_json(
    stream: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    wait: Duration,
) -> Option<Value> {
    use futures_util::StreamExt;
    loop {
        match tokio::time::timeout(wait, stream.next()).await {
            Ok(Some(Ok(tokio_tungstenite::tungstenite::Message::Text(text)))) => {
                return Some(serde_json::from_str(&text).unwrap());
            }
            Ok(Some(Ok(_))) => continue,
            _ => return None,
        }
    }
}

#[tokio::test]
async fn a_socket_follows_its_own_user_and_nobody_else() {
    let dir = scratch("socket");
    let users = Users::open(&dir.join("auth.db")).unwrap();
    users.add("ana@example.com", Role::Owner).unwrap();
    users.add("bo@example.com", Role::User).unwrap();
    let address = public_server(&dir, Path::new(".")).await;
    let ana_cookie = log_in(&address, &users, "ana@example.com").await;
    let bo_cookie = log_in(&address, &users, "bo@example.com").await;

    // Another site's page cannot open one, logged in or not.
    assert!(
        socket(&address, &bo_cookie, "", "https://evil.example")
            .await
            .is_err()
    );

    let mut ana = socket(&address, &ana_cookie, "", ORIGIN).await.unwrap();
    let mut bo = socket(&address, &bo_cookie, "", ORIGIN).await.unwrap();
    let ana_hello = next_json(&mut ana, Duration::from_secs(5)).await.unwrap();
    let bo_hello = next_json(&mut bo, Duration::from_secs(5)).await.unwrap();
    assert_eq!(ana_hello["type"], "hello");
    let (ana_first, bo_first) = (
        ana_hello["session"].as_str().unwrap().to_string(),
        bo_hello["session"].as_str().unwrap().to_string(),
    );
    assert_ne!(ana_first, bo_first);

    // Ana starts another session: her socket moves to it, Bo's stays put.
    let made = reqwest::Client::new()
        .post(format!("http://{address}/api/sessions"))
        .header("Cookie", &ana_cookie)
        .header("Origin", ORIGIN)
        .json(&serde_json::json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(made.status(), 201);
    let mut moved = false;
    while let Some(message) = next_json(&mut ana, Duration::from_secs(5)).await {
        if message["type"] == "hello" {
            assert_ne!(message["session"], ana_first.as_str());
            moved = true;
            break;
        }
    }
    assert!(moved, "Ana's socket follows her new session");
    while let Some(message) = next_json(&mut bo, Duration::from_millis(300)).await {
        assert_ne!(
            message["type"], "hello",
            "Bo's socket must not move: {message}"
        );
    }

    // And Bo cannot open a socket on Ana's first session by naming it.
    assert!(
        socket(
            &address,
            &bo_cookie,
            &format!("?session={ana_first}"),
            ORIGIN
        )
        .await
        .is_err()
    );
    std::fs::remove_dir_all(&dir).ok();
}
