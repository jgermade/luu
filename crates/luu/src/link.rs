//! One luu to another, over SSH, by key — the way intergate connects.
//!
//! **A link is one SSH connection, opened by whichever side can dial**: the
//! luu at home, behind NAT, dials the online one. It authenticates with its
//! own key (`<state>/id_ed25519`), which a user of the online server added to
//! their account, and it checks the online server's host key against the one
//! it pinned. Then it asks for a `tcpip-forward`, as `ssh -R` does, and the
//! online server opens one `forwarded-tcpip` channel per request its page
//! makes of `/h/<name>/…`, WebSockets included. The luu at home answers each
//! channel with its own router, **as a peer and never as loopback**: what a
//! peer may do is `[peer.<name>] may`, set on this machine.
//!
//! **Reconnection is intergate's.** The wait starts at 1 s and doubles up to
//! 30 s, and goes back to 1 s once a connection has lasted a minute. A
//! keepalive every 30 s finds a link NAT dropped without saying so. A new
//! link with the same user and name replaces the old one at once. A refusal
//! (a key on no account, a host key that is not the pinned one) is retried
//! every 30 s rather than given up on: from the page, adding the key on the
//! other side is what fixes it, and the link should follow without a click.
//!
//! See `RECORD/2026-11-03.one-luu-to-another-over-ssh.WIP.md`.

use std::collections::{BTreeMap, HashMap};
use std::net::SocketAddr;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use axum::Router;
use axum::body::Body;
use axum::extract::Request;
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use russh::keys::{HashAlg, PrivateKey, PrivateKeyWithHashAlg, PublicKey};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

use crate::provider::Peer;
use crate::users::Users;

/// This luu's own key, in the state directory.
pub const IDENTITY_FILE: &str = "id_ed25519";
/// The key a luu that accepts links answers with.
pub const HOST_KEY_FILE: &str = "ssh_host_ed25519_key";
/// intergate's port, and so luu's.
pub const DEFAULT_PORT: u16 = 2222;

const KEEPALIVE: Duration = Duration::from_secs(30);
const LONGEST_WAIT: Duration = Duration::from_secs(30);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

// ---- keys ------------------------------------------------------------------

/// A key from its file, or a new one written there: ed25519, from 32 bytes of
/// `/dev/urandom`, in OpenSSH's format so `ssh-keygen -lf` reads it, `0600`
/// from the moment it exists.
pub fn load_or_make(path: &Path, comment: &str) -> Result<PrivateKey> {
    if path.exists() {
        let text = crate::secret::read(
            path,
            "this luu's SSH key",
            "lets anyone who can read it act as this luu",
        )?;
        return PrivateKey::from_openssh(format!("{text}\n").as_bytes())
            .with_context(|| format!("reading the SSH key in {}", path.display()));
    }
    let seed = crate::users::random::<32>()?;
    let mut key = PrivateKey::from(russh::keys::ssh_key::private::Ed25519Keypair::from_seed(
        &seed,
    ));
    key.set_comment(comment);
    let text = key
        .to_openssh(russh::keys::ssh_key::LineEnding::LF)
        .context("writing an SSH key")?;
    crate::secret::write(path, &text)?;
    Ok(key)
}

/// This luu's identity: what another luu authorizes.
pub fn identity(dir: &Path) -> Result<PrivateKey> {
    load_or_make(&dir.join(IDENTITY_FILE), &format!("luu@{}", machine()))
}

/// The key this luu answers links with.
pub fn host_key(dir: &Path) -> Result<PrivateKey> {
    load_or_make(&dir.join(HOST_KEY_FILE), &format!("luu@{}", machine()))
}

/// This machine's name, for a key's comment: what a list of keys is read by.
fn machine() -> String {
    std::fs::read_to_string("/etc/hostname")
        .ok()
        .or_else(|| std::env::var("HOSTNAME").ok())
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "luu".to_string())
}

/// `ssh-ed25519 AAAA… luu@casa`, as `authorized_keys` writes one.
pub fn public_line(key: &PublicKey) -> String {
    key.to_openssh().unwrap_or_default()
}

/// `SHA256:…`, as `ssh-keygen -l` prints it.
pub fn fingerprint(key: &PublicKey) -> String {
    key.fingerprint(HashAlg::Sha256).to_string()
}

/// A public key as a person pastes it: one line, ed25519.
pub fn parse_public(text: &str) -> Result<PublicKey> {
    let key = PublicKey::from_openssh(text.trim())
        .context("not an OpenSSH public key: ssh-ed25519 AAAA… name")?;
    if key.algorithm() != russh::keys::Algorithm::Ed25519 {
        bail!(
            "only ssh-ed25519 keys link luus, and this one is {}",
            key.algorithm()
        );
    }
    Ok(key)
}

fn same_key(a: &PublicKey, b: &PublicKey) -> bool {
    a.key_data() == b.key_data()
}

/// The name a linked luu is shown under: `/h/<name>/`. Lowercase letters,
/// digits and `-`, up to 32, because it is a path segment and a word in a
/// list.
pub fn check_name(name: &str) -> Result<()> {
    let fits = !name.is_empty()
        && name.len() <= 32
        && !name.starts_with('-')
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if !fits {
        bail!("`{name}` is not a name for a luu: lowercase letters, digits and -, up to 32");
    }
    Ok(())
}

// ---- the side that accepts links -------------------------------------------

/// A luu linked to this one.
#[derive(Clone)]
pub struct Linked {
    /// Whose it is: the user whose key opened the link.
    pub user: String,
    /// What it is called here: `/h/<name>/`.
    pub name: String,
    /// The key it came with, `SHA256:…`.
    pub key: String,
    pub from: Option<SocketAddr>,
    /// Milliseconds since the epoch.
    pub since: u64,
    generation: u64,
    handle: russh::server::Handle,
}

/// Every link open now, by user and name. A new one for a user and a name
/// replaces the old, as intergate's does: a reconnect takes over at once.
#[derive(Default)]
pub struct Links {
    open: Mutex<HashMap<(String, String), Linked>>,
    next: AtomicU64,
    /// What answers a linked luu's own `direct-tcpip` channels: this server's
    /// `/api/infer` and nothing else, as the key's user. Set once the router
    /// exists.
    infer: OnceLock<Router>,
}

impl Links {
    fn open(&self) -> std::sync::MutexGuard<'_, HashMap<(String, String), Linked>> {
        self.open
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn put(&self, linked: Linked) -> Option<Linked> {
        self.open()
            .insert((linked.user.clone(), linked.name.clone()), linked)
    }

    fn forget(&self, user: &str, name: &str, generation: u64) {
        let mut open = self.open();
        let key = (user.to_string(), name.to_string());
        if open
            .get(&key)
            .is_some_and(|linked| linked.generation == generation)
        {
            open.remove(&key);
        }
    }

    /// A user's link by its name, or none: another user's is none too.
    pub fn get(&self, user: &str, name: &str) -> Option<Linked> {
        self.open()
            .get(&(user.to_string(), name.to_string()))
            .cloned()
    }

    /// A user's links, by name.
    pub fn of(&self, user: &str) -> Vec<Linked> {
        let mut linked: Vec<Linked> = self
            .open()
            .values()
            .filter(|linked| linked.user == user)
            .cloned()
            .collect();
        linked.sort_by(|a, b| a.name.cmp(&b.name));
        linked
    }

    fn all(&self) -> Vec<Linked> {
        self.open().values().cloned().collect()
    }

    /// The router a linked luu's `direct-tcpip` channels are answered by:
    /// the providers this server offers. See [`relay`].
    pub fn offer(&self, router: Router) {
        let _ = self.infer.set(router);
    }
}

/// Where this server accepts links, and the key it answers with.
#[derive(Debug, Clone)]
pub struct Listening {
    pub address: SocketAddr,
    pub host_key: String,
    pub fingerprint: String,
}

/// Accepts links on `bind`, from keys on `users`' accounts.
pub async fn listen(
    bind: &str,
    host: PrivateKey,
    users: Arc<Users>,
    links: Arc<Links>,
) -> Result<Listening> {
    let listening = Listening {
        address: SocketAddr::from(([0, 0, 0, 0], 0)),
        host_key: public_line(host.public_key()),
        fingerprint: fingerprint(host.public_key()),
    };
    let config = Arc::new(russh::server::Config {
        keys: vec![host],
        keepalive_interval: Some(KEEPALIVE),
        keepalive_max: 3,
        inactivity_timeout: None,
        auth_rejection_time: Duration::from_secs(1),
        auth_rejection_time_initial: Some(Duration::ZERO),
        ..Default::default()
    });
    let socket = tokio::net::TcpListener::bind(bind)
        .await
        .with_context(|| format!("binding {bind} for links"))?;
    let address = socket.local_addr().context("the address links come to")?;
    let mut server = Server {
        users: users.clone(),
        links: links.clone(),
    };
    tokio::spawn(async move {
        use russh::server::Server as _;
        if let Err(error) = server.run_on_socket(config, &socket).await {
            tracing::warn!(target: "luu::link", "the link listener stopped: {error}");
        }
    });
    tokio::spawn(sweep(users, links));
    Ok(Listening {
        address,
        ..listening
    })
}

/// Every 30 s, a link whose key left its account, or whose user was
/// suspended or removed, is closed: `luu user` runs in another process, and
/// this is how what it did reaches a link that is already open.
async fn sweep(users: Arc<Users>, links: Arc<Links>) {
    loop {
        tokio::time::sleep(KEEPALIVE).await;
        for linked in links.all() {
            let still = users
                .key_user(&linked.key)
                .ok()
                .flatten()
                .is_some_and(|user| user.id == linked.user);
            if !still {
                tracing::info!(target: "luu::link", name = linked.name, "closed: its key is on no account now");
                links.forget(&linked.user, &linked.name, linked.generation);
                let _ = linked
                    .handle
                    .disconnect(
                        russh::Disconnect::ByApplication,
                        "this key is on no account here now".into(),
                        "en".into(),
                    )
                    .await;
            }
        }
    }
}

#[derive(Clone)]
struct Server {
    users: Arc<Users>,
    links: Arc<Links>,
}

impl russh::server::Server for Server {
    type Handler = Connection;

    fn new_client(&mut self, from: Option<SocketAddr>) -> Connection {
        Connection {
            users: self.users.clone(),
            links: self.links.clone(),
            from,
            user: None,
            name: String::new(),
            key: String::new(),
            opened: Vec::new(),
        }
    }
}

/// One SSH connection: one luu, one user's key, one name.
struct Connection {
    users: Arc<Users>,
    links: Arc<Links>,
    from: Option<SocketAddr>,
    user: Option<String>,
    name: String,
    key: String,
    opened: Vec<u64>,
}

impl Connection {
    /// The user a key belongs to, when the name asked for is one.
    fn whose(&self, name: &str, key: &PublicKey) -> Option<crate::users::User> {
        check_name(name).ok()?;
        self.users.key_user(&fingerprint(key)).ok().flatten()
    }
}

impl russh::server::Handler for Connection {
    type Error = russh::Error;

    async fn auth_publickey_offered(
        &mut self,
        name: &str,
        key: &PublicKey,
    ) -> Result<russh::server::Auth, Self::Error> {
        Ok(match self.whose(name, key) {
            Some(_) => russh::server::Auth::Accept,
            None => russh::server::Auth::reject(),
        })
    }

    async fn auth_publickey(
        &mut self,
        name: &str,
        key: &PublicKey,
    ) -> Result<russh::server::Auth, Self::Error> {
        let Some(user) = self.whose(name, key) else {
            tracing::info!(target: "luu::link", name, key = fingerprint(key), "refused: a key on no account");
            return Ok(russh::server::Auth::reject());
        };
        self.user = Some(user.id);
        self.name = name.to_string();
        self.key = fingerprint(key);
        Ok(russh::server::Auth::Accept)
    }

    async fn tcpip_forward(
        &mut self,
        address: &str,
        port: &mut u32,
        session: &mut russh::server::Session,
    ) -> Result<bool, Self::Error> {
        let Some(user) = self.user.clone() else {
            return Ok(false);
        };
        // The name is the SSH user's; the address may repeat it, or be empty.
        if !address.is_empty() && address != self.name && address != "localhost" {
            return Ok(false);
        }
        *port = 80;
        let generation = self.links.next.fetch_add(1, Ordering::Relaxed) + 1;
        let linked = Linked {
            user,
            name: self.name.clone(),
            key: self.key.clone(),
            from: self.from,
            since: crate::session::now_ms(),
            generation,
            handle: session.handle(),
        };
        tracing::info!(target: "luu::link", name = linked.name, key = linked.key, from = ?linked.from, "linked");
        self.opened.push(generation);
        if let Some(old) = self.links.put(linked) {
            tokio::spawn(async move {
                let _ = old
                    .handle
                    .disconnect(
                        russh::Disconnect::ByApplication,
                        "replaced by a newer link with the same name".into(),
                        "en".into(),
                    )
                    .await;
            });
        }
        Ok(true)
    }

    /// A linked luu asking for this server's providers, as `ssh -L` would:
    /// answered by `/api/infer` only, as the key's user.
    async fn channel_open_direct_tcpip(
        &mut self,
        channel: russh::Channel<russh::server::Msg>,
        _host_to_connect: &str,
        _port_to_connect: u32,
        _originator_address: &str,
        _originator_port: u32,
        reply: russh::server::ChannelOpenHandle,
        _session: &mut russh::server::Session,
    ) -> Result<(), Self::Error> {
        match self.serves_direct() {
            Some(router) => {
                reply.accept().await;
                serve_channel(channel.into_stream(), router);
            }
            None => {
                reply
                    .reject(russh::ChannelOpenFailure::AdministrativelyProhibited)
                    .await;
            }
        }
        Ok(())
    }
}

impl Connection {
    fn serves_direct(&self) -> Option<Router> {
        self.user.as_ref()?;
        self.links.infer.get().cloned()
    }
}

/// Serves one channel's HTTP with `router`, upgrades included.
fn serve_channel<S>(stream: S, router: Router)
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let io = hyper_util::rt::TokioIo::new(stream);
        let service = hyper_util::service::TowerToHyperService::new(router);
        let _ = hyper::server::conn::http1::Builder::new()
            .serve_connection(io, service)
            .with_upgrades()
            .await;
    });
}

impl Drop for Connection {
    fn drop(&mut self) {
        if let Some(user) = &self.user {
            for generation in &self.opened {
                self.links.forget(user, &self.name, *generation);
            }
            if !self.opened.is_empty() {
                tracing::info!(target: "luu::link", name = self.name, "unlinked");
            }
        }
    }
}

fn bad_gateway(linked: &Linked, why: impl std::fmt::Display) -> Response {
    (
        StatusCode::BAD_GATEWAY,
        format!("{} (linked): {why}", linked.name),
    )
        .into_response()
}

async fn channel(
    linked: &Linked,
) -> Result<russh::ChannelStream<russh::server::Msg>, Box<Response>> {
    let channel = tokio::time::timeout(
        CONNECT_TIMEOUT,
        linked
            .handle
            .channel_open_forwarded_tcpip(linked.name.clone(), 80, "luu", 0),
    )
    .await
    .map_err(|_| Box::new(bad_gateway(linked, "no answer within 15 s")))?
    .map_err(|error| Box::new(bad_gateway(linked, error)))?;
    Ok(channel.into_stream())
}

/// A request the page made of a linked luu's API, carried over a channel of
/// its link.
pub async fn forward(
    linked: &Linked,
    method: Method,
    rest: &str,
    headers: &HeaderMap,
    body: Body,
) -> Response {
    let stream = match channel(linked).await {
        Ok(stream) => stream,
        Err(refused) => return *refused,
    };
    forward_on(stream, &linked.name, method, rest, headers, body)
        .await
        .unwrap_or_else(|why| bad_gateway(linked, why))
}

/// One HTTP request over one channel's stream, whichever way the channel
/// goes: `forwarded-tcpip` to a luu that dialled, `direct-tcpip` to the one
/// it dialled.
async fn forward_on<S>(
    stream: S,
    host: &str,
    method: Method,
    rest: &str,
    headers: &HeaderMap,
    body: Body,
) -> Result<Response, String>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let (mut sender, connection) =
        hyper::client::conn::http1::handshake::<_, Body>(hyper_util::rt::TokioIo::new(stream))
            .await
            .map_err(|error| error.to_string())?;
    tokio::spawn(async move {
        let _ = connection.await;
    });
    let mut asked = axum::http::Request::builder()
        .method(method)
        .uri(crate::hosts::without_token(rest));
    for (key, value) in headers {
        if !crate::hosts::NOT_FORWARDED.contains(key) {
            asked = asked.header(key, value);
        }
    }
    asked = asked.header(header::HOST, host);
    let asked = asked.body(body).map_err(|error| error.to_string())?;
    let answer = sender
        .send_request(asked)
        .await
        .map_err(|error| error.to_string())?;
    let (parts, body) = answer.into_parts();
    let mut response = Response::builder().status(parts.status);
    for (key, value) in &parts.headers {
        if !crate::hosts::NOT_RETURNED.contains(key) {
            response = response.header(key, value);
        }
    }
    response
        .body(Body::new(body))
        .map_err(|error| error.to_string())
}

/// The other end of a socket the page opened on a linked luu.
pub async fn connect(
    linked: &Linked,
    rest: &str,
) -> Result<
    tokio_tungstenite::WebSocketStream<russh::ChannelStream<russh::server::Msg>>,
    Box<Response>,
> {
    let stream = channel(linked).await?;
    let url = format!("ws://{}{}", linked.name, crate::hosts::without_token(rest));
    let request = url
        .as_str()
        .into_client_request()
        .map_err(|error| Box::new(bad_gateway(linked, error)))?;
    let (socket, _) = tokio::time::timeout(
        CONNECT_TIMEOUT,
        tokio_tungstenite::client_async(request, stream),
    )
    .await
    .map_err(|_| Box::new(bad_gateway(linked, "no answer within 15 s")))?
    .map_err(|error| match error {
        tokio_tungstenite::tungstenite::Error::Http(answer) => {
            let status =
                StatusCode::from_u16(answer.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
            let said = answer
                .body()
                .as_deref()
                .map(String::from_utf8_lossy)
                .unwrap_or_default()
                .trim()
                .to_string();
            Box::new((status, format!("{} (linked): {said}", linked.name)).into_response())
        }
        error => Box::new(bad_gateway(linked, error)),
    })?;
    Ok(socket)
}

/// What a page needs to link to this server: where to dial, and the key to
/// expect there. Served over HTTPS, so the trust in the key is the
/// certificate's on the address that was typed.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct LinkInfo {
    pub ssh: String,
    pub host_key: String,
}

/// `<url>/api/link.json`, asked by the side that will dial.
pub async fn probe(url: &str) -> Result<LinkInfo> {
    let base = url.trim().trim_end_matches('/');
    if !crate::provider::is_origin(base) {
        bail!("`{url}` is not a server's address: https://luu.example.com");
    }
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let answer = client
        .get(format!("{base}/api/link.json"))
        .send()
        .await
        .with_context(|| format!("asking {base}"))?;
    if !answer.status().is_success() {
        bail!(
            "{base} accepts no links ({}): its [server] ssh is not set",
            answer.status()
        );
    }
    let info: LinkInfo = answer
        .json()
        .await
        .with_context(|| format!("{base}/api/link.json"))?;
    parse_public(&info.host_key)?;
    Ok(info)
}

// ---- the side that dials ---------------------------------------------------

/// What Remote access shows of one peer.
#[derive(Debug, Clone, serde::Serialize, PartialEq, Eq)]
pub struct PeerStatus {
    /// `connecting`, `connected`, `retrying`, `refused` or `off`.
    pub state: &'static str,
    /// Milliseconds since the epoch, when it got there.
    pub since: u64,
    /// Why, for `retrying` and `refused`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub said: Option<String>,
    /// Seconds until the next try.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_in: Option<u64>,
}

/// The links this luu dials, and how each is doing.
#[derive(Default)]
pub struct Peers {
    statuses: Mutex<HashMap<String, PeerStatus>>,
    tasks: Mutex<HashMap<String, tokio::task::AbortHandle>>,
    router: OnceLock<Router>,
    identity: Mutex<Option<Arc<PrivateKey>>>,
    /// Each connected peer's SSH connection, for the `direct-tcpip` channels
    /// that reach the providers it offers. See [`relay`].
    handles: Mutex<HashMap<String, Arc<russh::client::Handle<Dialer>>>>,
    /// This server's own bearer token, when it has one: a channel's request
    /// passes the same gate a request on the port does.
    token: OnceLock<Option<String>>,
}

impl Peers {
    /// Called once the router exists: every `[peer.*]` that dials, dialled.
    pub fn start(
        self: &Arc<Self>,
        router: Router,
        token: Option<String>,
        peers: &BTreeMap<String, Peer>,
    ) {
        let _ = self.router.set(router);
        let _ = self.token.set(token);
        self.restart(peers);
    }

    /// This luu's key, made the first time something needs it.
    pub fn identity(&self) -> Result<Arc<PrivateKey>> {
        let mut held = self.identity.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(key) = held.as_ref() {
            return Ok(key.clone());
        }
        let dir = crate::config::dir().context("no state directory for this luu's key")?;
        let key = Arc::new(identity(&dir)?);
        *held = Some(key.clone());
        Ok(key)
    }

    /// Every dial stopped, and the ones `peers` says started again: what a
    /// write to Remote access ends with.
    pub fn restart(self: &Arc<Self>, peers: &BTreeMap<String, Peer>) {
        let Some(router) = self.router.get() else {
            return;
        };
        let identity = match peers.values().any(|peer| peer.dial) {
            false => None,
            true => match self.identity() {
                Ok(identity) => Some(identity),
                Err(error) => {
                    tracing::warn!(target: "luu::link", "no key to link with: {error:#}");
                    None
                }
            },
        };
        let token = self.token.get().cloned().flatten();
        let mut tasks = self.tasks.lock().unwrap_or_else(|p| p.into_inner());
        for (_, task) in tasks.drain() {
            task.abort();
        }
        self.handles
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clear();
        let mut statuses = self.statuses.lock().unwrap_or_else(|p| p.into_inner());
        statuses.clear();
        for (name, peer) in peers {
            let Some(identity) = identity.as_ref().filter(|_| peer.dial) else {
                statuses.insert(name.clone(), status("off", None, None));
                continue;
            };
            let gated = gated(router.clone(), peer.may.clone(), token.clone());
            let task = tokio::spawn(dial(
                name.clone(),
                peer.clone(),
                identity.clone(),
                gated,
                self.clone(),
            ));
            tasks.insert(name.clone(), task.abort_handle());
        }
    }

    pub fn statuses(&self) -> HashMap<String, PeerStatus> {
        self.statuses
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    /// A channel to `peer`'s own API, while it is linked.
    async fn channel(
        &self,
        peer: &str,
    ) -> Result<russh::ChannelStream<russh::client::Msg>, String> {
        let handle = self
            .handles
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(peer)
            .cloned()
            .ok_or_else(|| format!("{peer} is not linked now"))?;
        let channel = tokio::time::timeout(
            CONNECT_TIMEOUT,
            handle.channel_open_direct_tcpip("luu", 80, "luu", 0),
        )
        .await
        .map_err(|_| format!("{peer}: no answer within 15 s"))?
        .map_err(|error| format!("{peer}: {error}"))?;
        Ok(channel.into_stream())
    }

    fn keep(&self, peer: &str, handle: Option<Arc<russh::client::Handle<Dialer>>>) {
        let mut handles = self.handles.lock().unwrap_or_else(|p| p.into_inner());
        match handle {
            Some(handle) => handles.insert(peer.to_string(), handle),
            None => handles.remove(peer),
        };
    }

    fn set(&self, name: &str, state: PeerStatus) {
        self.statuses
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(name.to_string(), state);
    }
}

fn status(state: &'static str, said: Option<String>, retry_in: Option<Duration>) -> PeerStatus {
    PeerStatus {
        state,
        since: crate::session::now_ms(),
        said,
        retry_in: retry_in.map(|wait| wait.as_secs()),
    }
}

enum Ended {
    /// It was up, and went down.
    Closed(String),
    /// It never got up for a reason that waiting will not fix by itself.
    Refused(String),
    /// It never got up.
    Failed(String),
}

async fn dial(
    name: String,
    peer: Peer,
    identity: Arc<PrivateKey>,
    router: Router,
    peers: Arc<Peers>,
) {
    let mut wait = Duration::from_secs(1);
    loop {
        peers.set(&name, status("connecting", None, None));
        let started = Instant::now();
        let ended = once(&name, &peer, &identity, &router, &peers).await;
        if started.elapsed() > Duration::from_secs(60) {
            wait = Duration::from_secs(1);
        }
        let (state, said, this_wait) = match ended {
            Ended::Refused(said) => ("refused", said, LONGEST_WAIT),
            Ended::Closed(said) | Ended::Failed(said) => ("retrying", said, wait),
        };
        tracing::info!(target: "luu::link", peer = name, state, said, "link down");
        peers.set(&name, status(state, Some(said), Some(this_wait)));
        tokio::time::sleep(this_wait).await;
        wait = (wait * 2).min(LONGEST_WAIT);
    }
}

async fn once(
    name: &str,
    peer: &Peer,
    identity: &Arc<PrivateKey>,
    router: &Router,
    peers: &Arc<Peers>,
) -> Ended {
    let pinned = match parse_public(&peer.host_key) {
        Ok(pinned) => pinned,
        Err(error) => return Ended::Refused(format!("host-key: {error:#}")),
    };
    let matched = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let handler = Dialer {
        pinned,
        matched: matched.clone(),
        router: router.clone(),
    };
    let config = Arc::new(russh::client::Config {
        keepalive_interval: Some(KEEPALIVE),
        keepalive_max: 3,
        inactivity_timeout: None,
        ..Default::default()
    });
    let connected = tokio::time::timeout(
        CONNECT_TIMEOUT,
        russh::client::connect(config, peer.ssh.as_str(), handler),
    )
    .await;
    let mut session = match connected {
        Err(_) => return Ended::Failed(format!("{}: no answer within 15 s", peer.ssh)),
        Ok(Err(error)) => {
            return match matched.load(Ordering::Relaxed) {
                true => Ended::Failed(format!("{}: {error}", peer.ssh)),
                false => Ended::Refused(format!(
                    "{} answered with a key that is not the one pinned for it: if the server \
                     was reinstalled, link it again",
                    peer.ssh
                )),
            };
        }
        Ok(Ok(session)) => session,
    };
    let key = PrivateKeyWithHashAlg::new(identity.clone(), None);
    match session.authenticate_publickey(peer.name.clone(), key).await {
        Ok(result) if result.success() => {}
        Ok(_) => {
            return Ended::Refused(format!(
                "this luu's key, {}, is on no account there: add it in Account → Keys",
                fingerprint(identity.public_key())
            ));
        }
        Err(error) => return Ended::Failed(error.to_string()),
    }
    if let Err(error) = session.tcpip_forward(peer.name.clone(), 80).await {
        return Ended::Refused(format!("the link was refused: {error}"));
    }
    tracing::info!(target: "luu::link", peer = name, ssh = peer.ssh, "linked");
    peers.set(name, status("connected", None, None));
    // Kept for the channels that reach its providers, and watched: `Handle`
    // is a future that ends with the connection, but awaiting it would give
    // it away.
    let session = Arc::new(session);
    peers.keep(name, Some(session.clone()));
    while !session.is_closed() {
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    peers.keep(name, None);
    Ended::Closed("the connection closed".to_string())
}

pub struct Dialer {
    pinned: PublicKey,
    /// False once the server's key was not the pinned one, to tell that
    /// failure from a network's.
    matched: Arc<std::sync::atomic::AtomicBool>,
    router: Router,
}

impl russh::client::Handler for Dialer {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        key: &russh::keys::PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        let matches = match key {
            russh::keys::PublicKeyOrCertificate::PublicKey { key, .. } => {
                same_key(key, &self.pinned)
            }
            _ => false,
        };
        self.matched.store(matches, Ordering::Relaxed);
        Ok(matches)
    }

    async fn server_channel_open_forwarded_tcpip(
        &mut self,
        channel: russh::Channel<russh::client::Msg>,
        _connected_address: &str,
        _connected_port: u32,
        _originator_address: &str,
        _originator_port: u32,
        reply: russh::client::ChannelOpenHandle,
        _session: &mut russh::client::Session,
    ) -> Result<(), Self::Error> {
        reply.accept().await;
        serve_channel(channel.into_stream(), self.router.clone());
        Ok(())
    }
}

// ---- the providers another luu offers --------------------------------------

/// Where this process's backends reach a linked luu's providers, and the
/// secret their paths are signed with.
struct Relay {
    socket: std::path::PathBuf,
    secret: [u8; 32],
}

static RELAY: OnceLock<Relay> = OnceLock::new();

/// Who a path is for: `_` on a luu that dials, a user's id on a server with
/// users.
const DIALLER: &str = "_";

/// The path's signature for `owner`: the commands sessions run are luu's own
/// user, so they can connect to the socket like anything else luu's — a
/// socket's mode does not keep them out, and Landlock does not restrict
/// `connect` on one. A path for another owner cannot be made without the
/// secret, which never leaves this process.
fn mark(secret: &[u8; 32], owner: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::new()
        .chain_update(secret)
        .chain_update(owner.as_bytes())
        .finalize();
    digest[..16]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// The unix socket the backends dial for a provider another luu offers:
/// `<state>/sockets/links-<pid>.sock`, in a `0700` folder, itself `0600`.
/// Each request is `/via/<owner>/<mark>/<peer>/api/infer/…`, carried over
/// that peer's link: a `direct-tcpip` channel where this luu dialled, a
/// `forwarded-tcpip` one where the peer did. Nothing but `/api/infer` goes
/// through it.
pub async fn relay(links: Option<Arc<Links>>, peers: Arc<Peers>) -> Result<std::path::PathBuf> {
    use std::os::unix::fs::PermissionsExt;

    let dir = socket_dir()?;
    // One per process, and a process that ended does not remove its own: the
    // sockets of processes gone are removed here.
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            let pid = name
                .strip_prefix("links-")
                .and_then(|rest| rest.strip_suffix(".sock"))
                .and_then(|pid| pid.parse::<u32>().ok());
            if let Some(pid) = pid
                && pid != std::process::id()
                && !alive(pid)
            {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
    let socket = dir.join(format!("links-{}.sock", std::process::id()));
    let _ = std::fs::remove_file(&socket);
    let listener = tokio::net::UnixListener::bind(&socket)
        .with_context(|| format!("binding {}", socket.display()))?;
    std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600))?;
    let secret = crate::users::random::<32>()?;
    let _ = RELAY.set(Relay {
        socket: socket.clone(),
        secret,
    });
    let router = Router::new().route(
        "/via/{owner}/{mark}/{peer}/{*rest}",
        axum::routing::any(
            move |axum::extract::Path((owner, signed, peer, rest)): axum::extract::Path<(
                String,
                String,
                String,
                String,
            )>,
                  request: Request| {
                let links = links.clone();
                let peers = peers.clone();
                async move { via(links, peers, &owner, &signed, &peer, &rest, request).await }
            },
        ),
    );
    tokio::spawn(async move {
        if let Err(error) = axum::serve(listener, router).await {
            tracing::warn!(target: "luu::link", "the links' socket stopped: {error}");
        }
    });
    Ok(socket)
}

/// Whether a process is there: `kill(pid, 0)`, which sends nothing, and
/// fails with `ESRCH` only when there is no such process.
fn alive(pid: u32) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return false;
    };
    // SAFETY: signal 0 is a check, not a signal; nothing is delivered.
    let answer = unsafe { libc::kill(pid, 0) };
    answer == 0 || std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
}

/// Where the socket goes: `<state>/sockets`, `0700` — unless that path is
/// too long for a socket's (108 bytes on Linux, 104 on macOS, a long
/// `LUU_HOME` away), when it is `<temp>/luu-<uid>`, made `0700` and refused
/// unless it is this user's own folder and not a link.
fn socket_dir() -> Result<std::path::PathBuf> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    let longest = format!("links-{}.sock", u32::MAX).len();
    let state = crate::config::dir()
        .context("no state directory for the links' socket")?
        .join("sockets");
    let dir = match state.as_os_str().len() + 1 + longest < 100 {
        true => state,
        // SAFETY: geteuid has no preconditions and cannot fail.
        false => std::env::temp_dir().join(format!("luu-{}", unsafe { libc::geteuid() })),
    };
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let held = std::fs::symlink_metadata(&dir)?;
    // SAFETY: as above.
    if !held.is_dir() || held.uid() != unsafe { libc::geteuid() } {
        bail!("{} is not this user's own folder", dir.display());
    }
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
    Ok(dir)
}

async fn via(
    links: Option<Arc<Links>>,
    peers: Arc<Peers>,
    owner: &str,
    signed: &str,
    peer: &str,
    rest: &str,
    request: Request,
) -> Response {
    let refuse = |status, why: String| (status, why).into_response();
    let Some(relay) = RELAY.get() else {
        return refuse(StatusCode::SERVICE_UNAVAILABLE, "no relay".into());
    };
    if mark(&relay.secret, owner) != signed {
        return refuse(StatusCode::FORBIDDEN, "not this owner's path".into());
    }
    if !(rest == "api/infer" || rest.starts_with("api/infer/")) {
        return refuse(
            StatusCode::FORBIDDEN,
            "only /api/infer goes over a link from here".into(),
        );
    }
    let query = request
        .uri()
        .query()
        .map(|query| format!("?{query}"))
        .unwrap_or_default();
    let path = format!("/{rest}{query}");
    let (parts, body) = request.into_parts();
    let forwarded = match (owner, &links) {
        (DIALLER, _) => match peers.channel(peer).await {
            Ok(stream) => forward_on(stream, peer, parts.method, &path, &parts.headers, body).await,
            Err(why) => Err(why),
        },
        (user, Some(links)) => match links.get(user, peer) {
            Some(linked) => {
                return forward(&linked, parts.method, &path, &parts.headers, body).await;
            }
            None => Err(format!("no luu named {peer} is linked now")),
        },
        (_, None) => Err("this luu accepts no links".to_string()),
    };
    forwarded.unwrap_or_else(|why| refuse(StatusCode::BAD_GATEWAY, why))
}

/// Where a backend sends to reach `provider`, offered by `peer`, for
/// `owner` (`None` on a luu that dials): a base URL to hand the backend, and
/// the socket its client dials. `None` before the relay exists.
pub fn via_url(
    owner: Option<&str>,
    peer: &str,
    provider: &str,
) -> Option<(String, std::path::PathBuf)> {
    let relay = RELAY.get()?;
    let owner = owner.unwrap_or(DIALLER);
    // The host is the peer's name: a socket ignores it, and a backend names
    // itself after it (`openai@casa`), which is what the page should say.
    Some((
        format!(
            "http://{}/via/{owner}/{}/{peer}/api/infer/{provider}/to",
            peer.to_lowercase(),
            mark(&relay.secret, owner)
        ),
        relay.socket.clone(),
    ))
}

/// GET one of `peer`'s `/api/infer` paths through the relay, as JSON.
pub async fn ask_offered(owner: Option<&str>, peer: &str, path: &str) -> Result<serde_json::Value> {
    let relay = RELAY.get().context("this luu has no links' socket")?;
    let owner = owner.unwrap_or(DIALLER);
    let client = reqwest::Client::builder()
        .unix_socket(relay.socket.as_path())
        .timeout(Duration::from_secs(10))
        .build()?;
    let answer = client
        .get(format!(
            "http://luu/via/{owner}/{}/{peer}/api/infer{path}",
            mark(&relay.secret, owner)
        ))
        .send()
        .await
        .with_context(|| format!("asking {peer}"))?;
    let status = answer.status();
    let text = answer.text().await.unwrap_or_default();
    if !status.is_success() {
        bail!("{peer}: {} {}", status.as_u16(), text.trim());
    }
    Ok(serde_json::from_str(&text)?)
}

// ---- what a peer may do here -----------------------------------------------

/// What a request asks of this luu, as a peer's `may` names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Need {
    /// Reading anything, and making, running, approving and ending sessions.
    Sessions,
    /// A shell on this machine or in its container.
    Terminal,
    /// Every other write: providers, engines, runtimes, policies, the look.
    Settings,
    /// Remote access itself, other hosts, logins: never a peer's.
    Never,
}

impl Need {
    fn word(self) -> &'static str {
        match self {
            Self::Sessions => "sessions",
            Self::Terminal => "terminal",
            Self::Settings => "settings",
            Self::Never => "",
        }
    }
}

/// What `method` on `path` needs. Reads need `sessions`: the page reads
/// Settings to draw a session's pickers. Writes need `settings` unless they
/// are a session's, and a few paths are never a peer's — above all
/// `/api/peers`, since a peer that could change `may` could widen its own.
pub fn needs(method: &Method, path: &str) -> Need {
    let never = [
        "/api/peers",
        "/api/hosts",
        "/api/auth",
        "/api/setup",
        "/api/link",
        "/h/",
    ];
    if never.iter().any(|prefix| path.starts_with(prefix)) {
        return Need::Never;
    }
    if path == "/ws/terminal" {
        return Need::Terminal;
    }
    if path == "/ws" || path == "/ws/trace" {
        return Need::Sessions;
    }
    if !path.starts_with("/api/") {
        return Need::Never;
    }
    if matches!(*method, Method::GET | Method::HEAD) {
        return Need::Sessions;
    }
    // `/api/infer`: what a session here may send to, a peer may send to —
    // the providers this luu offers. See [`relay`].
    let a_sessions = [
        "/api/sessions",
        "/api/session/",
        "/api/workspace/",
        "/api/highlight",
        "/api/infer",
    ];
    match a_sessions.iter().any(|prefix| path.starts_with(prefix)) {
        true => Need::Sessions,
        false => Need::Settings,
    }
}

/// `router`, as a peer may use it: each request held to `may`, then made to
/// look like this machine's own page made it, which is what every check
/// below this one already knows how to read.
pub fn gated(router: Router, may: Vec<String>, token: Option<String>) -> Router {
    router.layer(axum::middleware::from_fn(
        move |request: Request, next: Next| {
            let may = may.clone();
            let token = token.clone();
            async move { peer_gate(&may, token.as_deref(), request, next).await }
        },
    ))
}

async fn peer_gate(
    may: &[String],
    token: Option<&str>,
    mut request: Request,
    next: Next,
) -> Response {
    let need = needs(request.method(), request.uri().path());
    let allowed = need != Need::Never && may.iter().any(|word| word == need.word());
    if !allowed {
        let why = match need {
            Need::Never => "this is never a linked luu's to reach".to_string(),
            need => format!(
                "this luu does not let its peer use `{}`: Settings → Remote access, on that \
                 machine, says what a peer may do",
                need.word()
            ),
        };
        return (StatusCode::FORBIDDEN, why).into_response();
    }
    let headers = request.headers_mut();
    for name in [
        header::COOKIE,
        header::AUTHORIZATION,
        header::ORIGIN,
        header::HOST,
        header::REFERER,
    ] {
        headers.remove(name);
    }
    headers.insert(header::HOST, HeaderValue::from_static("localhost"));
    headers.insert(header::ORIGIN, HeaderValue::from_static("http://localhost"));
    if let Some(token) = token
        && let Ok(value) = HeaderValue::from_str(&format!("Bearer {token}"))
    {
        headers.insert(header::AUTHORIZATION, value);
    }
    next.run(request).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("luu-link-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_key_is_made_once_0600_and_read_back_the_same() {
        use std::os::unix::fs::PermissionsExt;

        let dir = scratch("keys");
        let made = identity(&dir).unwrap();
        let path = dir.join(IDENTITY_FILE);
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let again = identity(&dir).unwrap();
        assert_eq!(
            public_line(made.public_key()),
            public_line(again.public_key())
        );
        assert!(public_line(made.public_key()).starts_with("ssh-ed25519 AAAA"));
        assert!(fingerprint(made.public_key()).starts_with("SHA256:"));
        // A key anyone can read is refused, as a token file is.
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(identity(&dir).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_pasted_key_is_ed25519_and_a_name_is_a_path_segment() {
        let dir = scratch("parse");
        let key = identity(&dir).unwrap();
        let line = public_line(key.public_key());
        assert!(parse_public(&format!("  {line}\n")).is_ok());
        assert!(parse_public("ssh-rsa AAAAB3NzaC1yc2E bob").is_err());
        assert!(parse_public("hello").is_err());
        assert!(check_name("casa").is_ok());
        assert!(check_name("casa-2").is_ok());
        for bad in ["", "Casa", "-casa", "casa/x", "a".repeat(33).as_str()] {
            assert!(check_name(bad).is_err(), "{bad}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_relay_path_is_one_owners_and_cannot_be_made_for_another() {
        let secret = [7u8; 32];
        assert_eq!(mark(&secret, "abc"), mark(&secret, "abc"));
        assert_ne!(mark(&secret, "abc"), mark(&secret, "abd"));
        assert_ne!(mark(&secret, "abc"), mark(&[8u8; 32], "abc"));
        assert_eq!(mark(&secret, "_").len(), 32);
    }

    #[test]
    fn what_a_peer_needs_by_route() {
        let get = Method::GET;
        let post = Method::POST;
        let put = Method::PUT;
        assert_eq!(needs(&get, "/api/sessions"), Need::Sessions);
        assert_eq!(needs(&post, "/api/sessions"), Need::Sessions);
        assert_eq!(needs(&post, "/api/sessions/abc/resume"), Need::Sessions);
        assert_eq!(needs(&put, "/api/session/folder"), Need::Sessions);
        assert_eq!(needs(&post, "/api/workspace/rename"), Need::Sessions);
        assert_eq!(needs(&get, "/ws"), Need::Sessions);
        assert_eq!(needs(&get, "/api/providers"), Need::Sessions);
        assert_eq!(needs(&put, "/api/providers"), Need::Settings);
        assert_eq!(needs(&post, "/api/engines/gemma/start"), Need::Settings);
        assert_eq!(needs(&put, "/api/terminal"), Need::Settings);
        assert_eq!(needs(&get, "/ws/terminal"), Need::Terminal);
        assert_eq!(needs(&get, "/api/infer"), Need::Sessions);
        assert_eq!(
            needs(&post, "/api/infer/gpu/to/chat/completions"),
            Need::Sessions
        );
        assert_eq!(needs(&get, "/api/peers"), Need::Never);
        assert_eq!(needs(&put, "/api/peers"), Need::Never);
        assert_eq!(needs(&put, "/api/hosts"), Need::Never);
        assert_eq!(needs(&get, "/h/other/api/sessions"), Need::Never);
        assert_eq!(needs(&get, "/index.html"), Need::Never);
    }
}
