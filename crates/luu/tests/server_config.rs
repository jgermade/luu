//! `[server]` in `config.toml`, and the `LUU_*` variables over it, deciding
//! how a real `luu serve` is reached. The binary, for `tests/hosts.rs`'s
//! reason: the file is read from `LUU_HOME`, and in-process that would be
//! every other test's environment too. See
//! `RECORD/2026-10-07.a-public-luu.completed.md`.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("the workspace root")
}

fn scratch() -> PathBuf {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("luu-server-{}-{n}", std::process::id()));
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

/// Starts `serve`, and answers the address it printed, or what it said on
/// stderr when it refused to start.
fn start(mut command: Command) -> Result<(std::process::Child, String), String> {
    let mut child = command
        .args(["serve", "--no-store"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("starting luu serve");
    let Some(stdout) = child.stdout.take() else {
        let _ = child.kill();
        let _ = child.wait();
        panic!("luu serve has no stdout");
    };
    let mut reader = BufReader::new(stdout);
    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line).unwrap_or(0) == 0 {
            let mut said = String::new();
            let _ = std::io::Read::read_to_string(
                child.stderr.as_mut().expect("its stderr"),
                &mut said,
            );
            let _ = child.wait();
            return Err(said);
        }
        if let Some(address) = line.trim().strip_prefix("luu serve → http://") {
            let address = address.to_string();
            std::thread::spawn(move || for _ in reader.lines() {});
            return Ok((child, address));
        }
    }
}

#[tokio::test]
async fn the_file_makes_a_server_private_and_the_environment_cannot_loosen_it() {
    let home = scratch();
    let token = home.join("token");
    let status = luu(&home)
        .args(["token", token.to_str().unwrap()])
        .status()
        .unwrap();
    assert!(status.success());
    std::fs::write(
        home.join("config.toml"),
        format!(
            "[server]\nexposure = \"private\"\nbind = \"127.0.0.1:0\"\ntoken-file = \"{}\"\n",
            token.display()
        ),
    )
    .unwrap();

    let (mut child, address) = start(luu(&home)).expect("a private server starts");
    let http = reqwest::Client::new();
    let bare = http
        .get(format!("http://{address}/api/settings"))
        .send()
        .await
        .unwrap();
    assert_eq!(bare.status(), 401, "private asks for the token");
    let secret = std::fs::read_to_string(&token).unwrap();
    let carried = http
        .get(format!("http://{address}/api/settings"))
        .bearer_auth(secret.trim())
        .send()
        .await
        .unwrap();
    assert_eq!(carried.status(), 200);
    let _ = child.kill();
    let _ = child.wait();

    // A variable overrides the file — and a contradiction is refused, not
    // read generously.
    let mut public = luu(&home);
    public
        .env("LUU_EXPOSURE", "loopback")
        .env("LUU_BIND", "0.0.0.0:0");
    let refused = start(public).expect_err("loopback on 0.0.0.0 is refused");
    assert!(refused.contains("not loopback"), "{refused}");
}

#[tokio::test]
async fn private_without_a_token_names_the_command_that_makes_one() {
    let home = scratch();
    std::fs::write(
        home.join("config.toml"),
        "[server]\nexposure = \"private\"\nbind = \"127.0.0.1:0\"\n",
    )
    .unwrap();
    let refused = start(luu(&home)).expect_err("private with no token is refused");
    assert!(refused.contains("luu token"), "{refused}");
}

#[test]
fn an_origin_that_is_not_one_is_a_load_error() {
    let home = scratch();
    std::fs::write(
        home.join("config.toml"),
        "[server]\norigin = \"https://luu.example.com/app\"\n",
    )
    .unwrap();
    let parsed = luu::provider::Config::from_toml(
        &std::fs::read_to_string(home.join("config.toml")).unwrap(),
        "config.toml",
    );
    assert!(
        matches!(parsed, Err(luu::provider::ConfigError::BadServer { .. })),
        "a path is not part of an origin"
    );
}

#[tokio::test]
async fn public_needs_an_origin_and_logs_in_with_a_code_luu_user_printed() {
    let home = scratch();
    std::fs::write(
        home.join("config.toml"),
        "[server]\nexposure = \"public\"\nbind = \"127.0.0.1:0\"\n",
    )
    .unwrap();
    let refused = start(luu(&home)).expect_err("public with no origin is refused");
    assert!(refused.contains("origin"), "{refused}");

    std::fs::write(
        home.join("config.toml"),
        "[server]\nexposure = \"public\"\nbind = \"127.0.0.1:0\"\n\
         origin = \"http://luu.test\"\n[auth]\nsession-hours = 2\n",
    )
    .unwrap();
    let added = luu(&home)
        .args(["user", "add", "Ana@Example.com", "--role", "owner"])
        .output()
        .unwrap();
    assert!(added.status.success(), "{added:?}");
    // The last owner stays.
    let demoted = luu(&home)
        .args(["user", "role", "ana@example.com", "user"])
        .output()
        .unwrap();
    assert!(!demoted.status.success());
    assert!(String::from_utf8_lossy(&demoted.stderr).contains("last owner"));
    let code = luu(&home)
        .args(["user", "code", "ana@example.com"])
        .output()
        .unwrap();
    assert!(code.status.success(), "{code:?}");
    let code = String::from_utf8(code.stdout).unwrap().trim().to_string();
    assert_eq!(code.len(), 6, "{code}");

    let (mut child, address) = start(luu(&home)).expect("a public server starts");
    let http = reqwest::Client::new();
    let bare = http
        .get(format!("http://{address}/api/settings"))
        .send()
        .await
        .unwrap();
    assert_eq!(bare.status(), 401, "public asks for a login");
    let login = http
        .post(format!("http://{address}/api/auth/login"))
        .header("Origin", "http://luu.test")
        .json(&serde_json::json!({ "email": "ana@example.com", "code": code }))
        .send()
        .await
        .unwrap();
    assert_eq!(login.status(), 204);
    let cookie = login.headers()["set-cookie"].to_str().unwrap().to_string();
    assert!(
        cookie.contains("Max-Age=7200"),
        "[auth] session-hours: {cookie}"
    );
    let cookie = cookie.split(';').next().unwrap().to_string();
    let me = http
        .get(format!("http://{address}/api/auth/me"))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(me.status(), 200);
    let me: serde_json::Value = me.json().await.unwrap();
    assert_eq!(me["role"], "owner");
    // A Settings write is still refused: roles reach Settings in step 3.
    let write = http
        .put(format!("http://{address}/api/tool-steps"))
        .header("Cookie", &cookie)
        .header("Origin", "http://luu.test")
        .json(&serde_json::json!({ "steps": 9 }))
        .send()
        .await
        .unwrap();
    assert_eq!(write.status(), 403);
    let _ = child.kill();
    let _ = child.wait();
    let mode = {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(home.join("auth.db"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777
    };
    assert_eq!(mode, 0o600);
}

#[tokio::test]
async fn luu_config_server_puts_a_server_online_and_keeps_the_rest_of_the_file() {
    let home = scratch();
    // A private server with a table of its own, as a container's first start
    // leaves it, plus a setting nobody should lose.
    std::fs::write(
        home.join("config.toml"),
        "[server]\nexposure = \"private\"\nbind = \"127.0.0.1:0\"\ntoken-file = \"/nonexistent/token\"\n\
         [sessions]\nmax-live = 2\n",
    )
    .unwrap();

    // Without a terminal, what no flag said is an error naming the flag.
    let asked = luu(&home)
        .args(["config", "server"])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(!asked.status.success());
    assert!(String::from_utf8_lossy(&asked.stderr).contains("--origin"));

    let set = luu(&home)
        .args(["config", "server", "--origin", "http://luu.test"])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(set.status.success(), "{set:?}");
    assert!(
        String::from_utf8(set.stdout)
            .unwrap()
            .contains("restart it")
    );
    let written = std::fs::read_to_string(home.join("config.toml")).unwrap();
    assert!(written.contains("exposure = \"public\""), "{written}");
    assert!(!written.contains("token-file"), "{written}");
    assert!(written.contains("max-live = 2"), "{written}");
    // The bind it had is kept: a test server's `127.0.0.1:0`.
    assert!(written.contains("127.0.0.1:0"), "{written}");
    // Links from other luus are on by default, on intergate's port; moved to
    // an ephemeral one here, so no two tests and no sshd ever share it.
    assert!(written.contains("ssh = \"0.0.0.0:2222\""), "{written}");
    let moved = luu(&home)
        .args([
            "config",
            "server",
            "--origin",
            "http://luu.test",
            "--ssh",
            "127.0.0.1:0",
        ])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(moved.status.success(), "{moved:?}");

    // The server it describes starts, with nobody yet: the page says so.
    let (mut child, address) = start(luu(&home)).expect("an online server starts");
    let me = reqwest::get(format!("http://{address}/api/auth/me.json"))
        .await
        .unwrap();
    assert_eq!(me.status(), 401);
    let me: serde_json::Value = me.json().await.unwrap();
    assert_eq!(me["users"], true);
    assert_eq!(me["owner"], false);
    // And where a luu dials to link here, with the key to pin.
    let info: serde_json::Value = reqwest::get(format!("http://{address}/api/link.json"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        info["ssh"].as_str().unwrap().starts_with("luu.test:"),
        "{info}"
    );
    assert!(
        info["host_key"]
            .as_str()
            .unwrap()
            .starts_with("ssh-ed25519 ")
    );
    let _ = child.kill();
    let _ = child.wait();

    // A setup link needs no owner, and names the origin.
    let link = luu(&home).args(["config", "setup-url"]).output().unwrap();
    assert!(link.status.success(), "{link:?}");
    let link = String::from_utf8(link.stdout).unwrap();
    assert!(link.starts_with("http://luu.test/?setup="), "{link}");
    // Not a terminal and no --qr: the link alone, for a script to read.
    assert_eq!(link.lines().count(), 1, "{link}");
}

#[path = "support/smtp.rs"]
mod smtp;

#[tokio::test]
async fn luu_config_owner_proves_the_mail_and_the_mailbox_with_one_code() {
    let home = scratch();
    std::fs::write(
        home.join("config.toml"),
        "[server]\nexposure = \"public\"\nbind = \"127.0.0.1:0\"\norigin = \"http://luu.test\"\n",
    )
    .unwrap();
    let inbox = smtp::start().await;

    // A mail server that is not there: the send fails, and nothing is saved.
    let refused = luu(&home)
        .args([
            "config",
            "owner",
            "--email",
            "ana@example.com",
            "--smtp-url",
            "smtp://127.0.0.1:1",
            "--smtp-from",
            "luu <luu@example.com>",
        ])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(!refused.status.success());
    assert!(String::from_utf8_lossy(&refused.stderr).contains("nothing was saved"));
    assert!(
        !std::fs::read_to_string(home.join("config.toml"))
            .unwrap()
            .contains("[auth.smtp]")
    );

    // The fake one, with a user and a password from a file a script would
    // hand it: a code arrives, and typed back it makes the owner and saves
    // the mail.
    let secret = home.join("given-password");
    std::fs::write(&secret, "s3cret-smtp\n").unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&secret, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    let mut child = luu(&home)
        .args([
            "config",
            "owner",
            "--email",
            "Ana@Example.com",
            "--smtp-url",
            &inbox.url,
            "--smtp-from",
            "luu <luu@example.com>",
            "--smtp-user",
            "luu",
            "--smtp-password-file",
            secret.to_str().unwrap(),
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mail = inbox.wait_for("ana@example.com").await;
    let code = mail.code().expect("a code in the subject");
    assert!(mail.data.contains("http://luu.test"), "{}", mail.data);
    assert_eq!(mail.password.as_deref(), Some("s3cret-smtp"));
    {
        use std::io::Write;
        let mut stdin = child.stdin.take().unwrap();
        writeln!(stdin, "{code}").unwrap();
    }
    let done = child.wait_with_output().unwrap();
    assert!(done.status.success(), "{done:?}");
    let written = std::fs::read_to_string(home.join("config.toml")).unwrap();
    assert!(written.contains("[auth.smtp]"), "{written}");
    assert!(written.contains(&inbox.url), "{written}");
    // The password is not in config.toml: it is in smtp.password, 0600, which
    // the file names.
    assert!(!written.contains("s3cret"), "{written}");
    assert!(written.contains("password-file"), "{written}");
    let saved = home.join("smtp.password");
    assert_eq!(
        std::fs::read_to_string(&saved).unwrap().trim(),
        "s3cret-smtp"
    );
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&saved).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }
    let listed = luu(&home).args(["user", "list"]).output().unwrap();
    let listed = String::from_utf8(listed.stdout).unwrap();
    assert!(
        listed
            .lines()
            .any(|line| line.starts_with("ana@example.com") && line.contains("owner")),
        "{listed}"
    );

    // Other mail, with the password a cluster mounts: read where it is, with
    // the mode a Kubernetes Secret has, and never copied.
    let mounted = home.join("run-secrets").join("smtp-password");
    std::fs::create_dir_all(mounted.parent().unwrap()).unwrap();
    std::fs::write(&mounted, "m0unted\n").unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&mounted, std::fs::Permissions::from_mode(0o644)).unwrap();
    }
    let reference = format!("file:{}", mounted.display());
    let env = luu(&home)
        .args(["config", "mail", "--smtp-url", &inbox.url])
        .args(["--smtp-from", "luu@example.com", "--smtp-user", "luu"])
        .args(["--smtp-password-ref", "env:SMTP_PASSWORD"])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(!env.status.success());
    assert!(String::from_utf8_lossy(&env.stderr).contains("inherits"));
    let mut child = luu(&home)
        .args(["config", "mail", "--smtp-url", &inbox.url])
        .args(["--smtp-from", "luu@example.com", "--smtp-user", "luu"])
        .args(["--smtp-password-ref", &reference])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let before = inbox.sent().len();
    let mail = loop {
        let sent = inbox.sent();
        if sent.len() > before {
            break sent.last().unwrap().clone();
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    };
    assert_eq!(mail.password.as_deref(), Some("m0unted"));
    {
        use std::io::Write;
        let mut stdin = child.stdin.take().unwrap();
        writeln!(stdin, "{}", mail.code().unwrap()).unwrap();
    }
    let done = child.wait_with_output().unwrap();
    assert!(done.status.success(), "{done:?}");
    let written = std::fs::read_to_string(home.join("config.toml")).unwrap();
    assert!(
        written.contains(&format!("password-ref = \"{reference}\"")),
        "{written}"
    );
    assert!(!written.contains("password-file"), "{written}");
    assert!(!written.contains("m0unted"), "{written}");
    assert!(!home.join("smtp.password").exists());

    // And now there is an owner: no second, and no setup link.
    let again = luu(&home)
        .args(["config", "owner", "--email", "eve@example.com"])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(!again.status.success());
    let link = luu(&home).args(["config", "setup-url"]).output().unwrap();
    assert!(!link.status.success());
}

#[tokio::test]
async fn setup_in_the_page_makes_the_owner_and_codes_come_by_email_after() {
    let home = scratch();
    std::fs::write(
        home.join("config.toml"),
        "[server]\nexposure = \"public\"\nbind = \"127.0.0.1:0\"\norigin = \"http://luu.test\"\n",
    )
    .unwrap();
    let inbox = smtp::start().await;
    let (mut child, address) = start(luu(&home)).expect("an online server starts");
    let http = reqwest::Client::new();
    let post = |path: &str, body: serde_json::Value| {
        http.post(format!("http://{address}{path}"))
            .header("Origin", "http://luu.test")
            .json(&body)
            .send()
    };

    let me: serde_json::Value = reqwest::get(format!("http://{address}/api/auth/me.json"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        (me["owner"].clone(), me["mail"].clone()),
        (false.into(), false.into())
    );

    // No key, no setup.
    let refused = post(
        "/api/setup/send",
        serde_json::json!({ "key": "nope", "email": "ana@example.com", "url": inbox.url, "from": "luu@example.com" }),
    )
    .await
    .unwrap();
    assert_eq!(refused.status(), 403);

    let link = luu(&home).args(["config", "setup-url"]).output().unwrap();
    let link = String::from_utf8(link.stdout).unwrap();
    let key = link.trim().rsplit("setup=").next().unwrap().to_string();

    // Mail settings that cannot send: said, and nothing saved.
    let failed = post(
        "/api/setup/send",
        serde_json::json!({ "key": key, "email": "ana@example.com", "url": "smtp://127.0.0.1:1", "from": "luu@example.com" }),
    )
    .await
    .unwrap();
    assert_eq!(failed.status(), 502);
    assert!(failed.text().await.unwrap().contains("nothing was saved"));

    // The owner's code is the test of the mail.
    let sent = post(
        "/api/setup/send",
        serde_json::json!({ "key": key, "email": "Ana@Example.com", "url": inbox.url, "from": "luu <luu@example.com>" }),
    )
    .await
    .unwrap();
    assert_eq!(sent.status(), 202, "{:?}", sent.text().await);
    let code = inbox.wait_for("ana@example.com").await.code().unwrap();
    let wrong = if code == "000000" { "000001" } else { "000000" };
    let bad = post(
        "/api/setup/claim",
        serde_json::json!({ "key": key, "email": "ana@example.com", "code": wrong }),
    )
    .await
    .unwrap();
    assert_eq!(bad.status(), 401);
    let claimed = post(
        "/api/setup/claim",
        serde_json::json!({ "key": key, "email": "ana@example.com", "code": code }),
    )
    .await
    .unwrap();
    assert_eq!(claimed.status(), 204);
    let cookie = claimed.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();
    let me: serde_json::Value = http
        .get(format!("http://{address}/api/auth/me.json"))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(me["role"], "owner");
    assert_eq!(me["mail"], true);
    let written = std::fs::read_to_string(home.join("config.toml")).unwrap();
    assert!(
        written.contains("[auth.smtp]") && written.contains(&inbox.url),
        "{written}"
    );

    // The link is spent.
    let spent = post(
        "/api/setup/send",
        serde_json::json!({ "key": key, "email": "eve@example.com" }),
    )
    .await
    .unwrap();
    assert_eq!(spent.status(), 403);

    // After: a user asks for a code and gets it by email; a stranger gets
    // the same answer and no email.
    assert!(
        luu(&home)
            .args(["user", "add", "bo@example.com"])
            .output()
            .unwrap()
            .status
            .success()
    );
    let asked = post(
        "/api/auth/code",
        serde_json::json!({ "email": "bo@example.com" }),
    )
    .await
    .unwrap();
    let stranger = post(
        "/api/auth/code",
        serde_json::json!({ "email": "eve@example.com" }),
    )
    .await
    .unwrap();
    assert_eq!(
        (asked.status().as_u16(), stranger.status().as_u16()),
        (202, 202)
    );
    assert_eq!(asked.text().await.unwrap(), stranger.text().await.unwrap());
    let code = inbox.wait_for("bo@example.com").await.code().unwrap();
    let login = post(
        "/api/auth/login",
        serde_json::json!({ "email": "bo@example.com", "code": code }),
    )
    .await
    .unwrap();
    assert_eq!(login.status(), 204);
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert!(
        !inbox
            .sent()
            .iter()
            .any(|sent| sent.to.iter().any(|to| to.contains("eve@"))),
        "a stranger got mail"
    );
    let _ = child.kill();
    let _ = child.wait();
}
