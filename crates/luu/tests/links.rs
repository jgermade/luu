//! One luu linked to another, over SSH by key: two real `luu serve`, an
//! online one with users and a local one that dials it, the way a person
//! would set them up — `luu link add`, `luu user key add` — and the online
//! page reaching the local luu through `/h/<name>/`. See
//! `RECORD/2026-11-03.one-luu-to-another-over-ssh.WIP.md`.

use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::Value;

const ORIGIN: &str = "http://luu.test";

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("the workspace root")
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("luu-links-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    dir
}

fn luu(home: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_luu"));
    command
        .current_dir(root())
        .env("LUU_HOME", home)
        .env_remove("LUU_BIND")
        .env_remove("LUU_EXPOSURE")
        .env_remove("LUU_AUTH_TOKEN_FILE")
        .env_remove("LUU_ORIGIN")
        .env_remove("LUU_AUTH_DB");
    command
}

fn run(home: &Path, args: &[&str]) -> String {
    let out = luu(home).args(args).output().expect("running luu");
    assert!(
        out.status.success(),
        "luu {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

/// A running `luu serve`, killed when dropped, with its stderr kept.
struct Serving {
    child: Child,
    address: String,
    said: Arc<Mutex<String>>,
}

impl Drop for Serving {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn start(home: &Path) -> Serving {
    let (child, address, said) = spawn(home);
    Serving {
        child,
        address,
        said,
    }
}

fn spawn(home: &Path) -> (Child, String, Arc<Mutex<String>>) {
    let mut child = luu(home)
        .args(["serve", "--no-store"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("starting luu serve");
    let said = Arc::new(Mutex::new(String::new()));
    let mut stderr = child.stderr.take().unwrap();
    let kept = said.clone();
    std::thread::spawn(move || {
        let mut buffer = [0u8; 4096];
        while let Ok(n) = stderr.read(&mut buffer) {
            if n == 0 {
                break;
            }
            kept.lock()
                .unwrap()
                .push_str(&String::from_utf8_lossy(&buffer[..n]));
        }
    });
    let mut reader = BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();
    let mut found = None;
    while reader.read_line(&mut line).unwrap_or(0) > 0 {
        if let Some(address) = line.trim().strip_prefix("luu serve → http://") {
            found = Some(address.to_string());
            break;
        }
        line.clear();
    }
    match found {
        Some(address) => {
            std::thread::spawn(move || for _ in reader.lines() {});
            (child, address, said)
        }
        None => {
            let _ = child.wait();
            std::thread::sleep(Duration::from_millis(100));
            panic!("luu serve did not start: {}", said.lock().unwrap());
        }
    }
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

async fn log_in(address: &str, home: &Path, email: &str) -> String {
    let code = run(home, &["user", "code", email]).trim().to_string();
    let response = reqwest::Client::new()
        .post(format!("http://{address}/api/auth/login"))
        .header("Origin", ORIGIN)
        .json(&serde_json::json!({ "email": email, "code": code }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 204);
    response.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string()
}

async fn get(address: &str, path: &str, cookie: &str) -> reqwest::Response {
    reqwest::Client::new()
        .get(format!("http://{address}{path}"))
        .header("Cookie", cookie)
        .send()
        .await
        .unwrap()
}

/// Waits until `cookie`'s user sees `name` among their hosts.
async fn linked(address: &str, cookie: &str, name: &str) -> bool {
    for _ in 0..150 {
        let hosts: Value = get(address, "/api/hosts", cookie)
            .await
            .json()
            .await
            .unwrap();
        if hosts["hosts"].get(name).is_some() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    false
}

async fn peer_state(address: &str, peer: &str) -> String {
    let peers: Value = reqwest::get(format!("http://{address}/api/peers"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    peers["status"][peer]["state"]
        .as_str()
        .unwrap_or("")
        .to_string()
}

#[tokio::test]
async fn the_online_page_reaches_a_luu_at_home_by_key_and_nobody_else_does() {
    let online = scratch("online");
    let home = scratch("home");
    let stranger = scratch("stranger");
    let port = free_port();
    std::fs::write(
        online.join("config.toml"),
        format!(
            "[server]\nexposure = \"public\"\nbind = \"127.0.0.1:0\"\norigin = \"{ORIGIN}\"\n\
             ssh = \"127.0.0.1:{port}\"\nssh-public = \"127.0.0.1:{port}\"\n"
        ),
    )
    .unwrap();
    run(
        &online,
        &["user", "add", "ana@example.com", "--role", "owner"],
    );
    run(&online, &["user", "add", "bo@example.com"]);
    let server = start(&online);
    let address = server.address.clone();
    assert!(
        server
            .said
            .lock()
            .unwrap()
            .contains("links — ssh on 127.0.0.1"),
        "{}",
        server.said.lock().unwrap()
    );

    // What a luu asks before linking: where, and the key to expect.
    let info: Value = reqwest::get(format!("http://{address}/api/link.json"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(info["ssh"], format!("127.0.0.1:{port}"));
    assert!(
        info["host_key"]
            .as_str()
            .unwrap()
            .starts_with("ssh-ed25519 ")
    );

    // At home: link to it, as `casa`. The key is pinned from its page.
    std::fs::write(
        home.join("config.toml"),
        "[server]\nbind = \"127.0.0.1:0\"\n",
    )
    .unwrap();
    let added = run(
        &home,
        &[
            "link",
            "add",
            &format!("http://{address}"),
            "--name",
            "casa",
            "--as",
            "online",
        ],
    );
    assert!(added.contains("[peer.online]"), "{added}");
    let written = std::fs::read_to_string(home.join("config.toml")).unwrap();
    assert!(written.contains("host-key = \"ssh-ed25519 "), "{written}");
    let key = run(&home, &["link", "key"]).trim().to_string();
    assert!(key.starts_with("ssh-ed25519 "), "{key}");
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(home.join("id_ed25519"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
    }

    // Before ana adds the key, the link is refused, and says why.
    let local = start(&home);
    let mut refused = String::new();
    for _ in 0..100 {
        refused = peer_state(&local.address, "online").await;
        if refused == "refused" {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(refused, "refused", "{}", local.said.lock().unwrap());
    drop(local);

    // Ana adds it to her account; the luu at home links, and is hers alone.
    run(&online, &["user", "key", "add", "ana@example.com", &key]);
    let local = start(&home);
    let ana = log_in(&address, &online, "ana@example.com").await;
    let bo = log_in(&address, &online, "bo@example.com").await;
    assert!(
        linked(&address, &ana, "casa").await,
        "online: {}\nhome: {}",
        server.said.lock().unwrap(),
        local.said.lock().unwrap()
    );
    assert_eq!(peer_state(&local.address, "online").await, "connected");
    let bo_hosts: Value = get(&address, "/api/hosts", &bo).await.json().await.unwrap();
    assert!(bo_hosts["hosts"].as_object().unwrap().is_empty());

    // Through the online page, the local luu's own API answers: its settings
    // say where it was started, which is home's state and not online's.
    let sessions = get(&address, "/h/casa/api/sessions", &ana).await;
    assert_eq!(sessions.status(), 200);
    // `/api/auth` is never a peer's: the page under /h/casa/ asks it, gets a
    // refusal, and draws itself as a page with no users, which home is.
    let me = get(&address, "/h/casa/api/auth/me.json", &ana).await;
    assert_eq!(me.status(), 403);
    let settings = get(&address, "/h/casa/api/settings", &ana).await;
    assert_eq!(settings.status(), 200);

    // Nobody else reaches it, and it reads like a luu that is not there.
    let as_bo = get(&address, "/h/casa/api/sessions", &bo).await;
    assert_eq!(as_bo.status(), 404);
    let nobody = reqwest::get(format!("http://{address}/h/casa/api/sessions"))
        .await
        .unwrap();
    assert_eq!(nobody.status(), 401);

    // What a peer may do is home's to say: sessions, and no Settings write,
    // and never Remote access.
    let write = reqwest::Client::new()
        .put(format!("http://{address}/h/casa/api/providers"))
        .header("Cookie", &ana)
        .header("Origin", ORIGIN)
        .json(&serde_json::json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(write.status(), 403);
    assert!(write.text().await.unwrap().contains("settings"));
    let peers = get(&address, "/h/casa/api/peers", &ana).await;
    assert_eq!(peers.status(), 403);

    // The protocol socket, through the link: home's session says hello.
    {
        use futures_util::StreamExt;
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;
        let mut request = format!("ws://{address}/h/casa/ws")
            .into_client_request()
            .unwrap();
        request.headers_mut().insert("Cookie", ana.parse().unwrap());
        request
            .headers_mut()
            .insert("Origin", ORIGIN.parse().unwrap());
        let (mut socket, _) = tokio_tungstenite::connect_async(request).await.unwrap();
        let hello = loop {
            match tokio::time::timeout(Duration::from_secs(5), socket.next()).await {
                Ok(Some(Ok(tokio_tungstenite::tungstenite::Message::Text(text)))) => {
                    break serde_json::from_str::<Value>(&text).unwrap();
                }
                Ok(Some(Ok(_))) => continue,
                other => panic!("no hello through the link: {other:?}"),
            }
        };
        assert_eq!(hello["type"], "hello");
    }

    // A luu whose key is on nobody's account is refused at the handshake.
    std::fs::write(
        stranger.join("config.toml"),
        written.replace("name = \"casa\"", "name = \"other\""),
    )
    .unwrap();
    let other = start(&stranger);
    let mut state = String::new();
    for _ in 0..100 {
        state = peer_state(&other.address, "online").await;
        if state == "refused" {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(state, "refused");
    drop(other);

    // The online server goes away and comes back: home links again by itself.
    drop(server);
    tokio::time::sleep(Duration::from_millis(500)).await;
    let server = start(&online);
    let address = server.address.clone();
    let ana = log_in(&address, &online, "ana@example.com").await;
    assert!(
        linked(&address, &ana, "casa").await,
        "home: {}",
        local.said.lock().unwrap()
    );
    let again = get(&address, "/h/casa/api/sessions", &ana).await;
    assert_eq!(again.status(), 200);

    drop(local);
    drop(server);
    for dir in [online, home, stranger] {
        let _ = std::fs::remove_dir_all(dir);
    }
}
