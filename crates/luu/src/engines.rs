//! Model servers luu starts: `llama-server` and `ollama serve`, as children of
//! `luu serve` that die with it.
//!
//! Three questions, one section each: **which binary** (found on this machine,
//! luu's own copy, or a path the file names), **its life** (started when a
//! session needs it, stopped with the server), and **luu's own copy** (an
//! official release, by tag, checked against the digest GitHub publishes for
//! it). See `RECORD/2026-09-28.a-model-server-luu-starts.completed.md`.

use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::Serialize;
use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::sync::Mutex;

use crate::provider::{Engine, EngineKind};

/// The releases luu downloads when the table names no `version`. Pinned rather
/// than `latest`: llama.cpp ships a build every few hours, and a copy that
/// moved under the machine is a copy nobody can compare a run against.
pub const LLAMA_VERSION: &str = "b11236";
pub const OLLAMA_VERSION: &str = "v0.34.4";
/// `mlx-serve`, the MLX engine: a native binary with a GitHub release and a
/// digest, measured against `mlx_lm.server` on this repository's M-series
/// machine and chosen over it — see the engines record.
pub const MLX_SERVE_VERSION: &str = "v26.9.6";

/// Lines of a server's output kept for the page: enough to show why it died.
const LOG_LINES: usize = 200;
const DEFAULT_READY_SECS: u64 = 120;

impl EngineKind {
    pub fn as_str(self) -> &'static str {
        match self {
            EngineKind::Llama => "llama",
            EngineKind::Ollama => "ollama",
            EngineKind::Mlx => "mlx",
        }
    }

    /// The executable's file name — and, from the page, the only name a path
    /// may end in. See [`accepts_from_page`].
    pub fn binary_name(self) -> &'static str {
        match (self, cfg!(windows)) {
            (EngineKind::Llama, false) => "llama-server",
            (EngineKind::Llama, true) => "llama-server.exe",
            (EngineKind::Ollama, false) => "ollama",
            (EngineKind::Ollama, true) => "ollama.exe",
            (EngineKind::Mlx, _) => "mlx-serve",
        }
    }

    pub fn default_port(self) -> u16 {
        match self {
            EngineKind::Llama => 8080,
            EngineKind::Ollama => 11434,
            EngineKind::Mlx => 11234,
        }
    }

    pub fn pinned(self) -> &'static str {
        match self {
            EngineKind::Llama => LLAMA_VERSION,
            EngineKind::Ollama => OLLAMA_VERSION,
            EngineKind::Mlx => MLX_SERVE_VERSION,
        }
    }

    fn repo(self) -> &'static str {
        match self {
            EngineKind::Llama => "ggml-org/llama.cpp",
            EngineKind::Ollama => "ollama/ollama",
            EngineKind::Mlx => "ddalcu/mlx-serve",
        }
    }

    /// What answers once the model is loaded. llama-server's `/health` is 503
    /// while it loads and 200 after; ollama's `/` is 200 as soon as it listens,
    /// because it loads a model per request.
    fn health(self) -> &'static str {
        match self {
            EngineKind::Llama | EngineKind::Mlx => "/health",
            EngineKind::Ollama => "/",
        }
    }

    /// The URL a profile for this engine sends to.
    pub fn url(self, port: u16) -> String {
        match self {
            EngineKind::Llama | EngineKind::Mlx => format!("http://127.0.0.1:{port}/v1"),
            EngineKind::Ollama => format!("http://127.0.0.1:{port}"),
        }
    }

    /// The release asset for this machine, by the names each project uses.
    pub fn asset(self, version: &str, variant: Option<&str>) -> Result<String, String> {
        asset_for(
            self,
            version,
            variant,
            std::env::consts::OS,
            std::env::consts::ARCH,
        )
    }
}

fn asset_for(
    kind: EngineKind,
    version: &str,
    variant: Option<&str>,
    os: &str,
    arch: &str,
) -> Result<String, String> {
    let unknown = || format!("no {} release is published for {os}/{arch}", kind.as_str());
    match kind {
        // One asset, and one platform: MLX is Apple Silicon's. The tag is
        // not in the name.
        EngineKind::Mlx => match (os, arch, variant) {
            ("macos", "aarch64", None) => Ok("mlx-serve-bin-macos-arm64.tar.gz".into()),
            ("macos", "aarch64", Some(v)) => Err(format!("mlx-serve publishes no `{v}` build")),
            _ => Err(format!(
                "MLX runs on Apple Silicon, and this is {os}/{arch}"
            )),
        },
        EngineKind::Llama => {
            let arch = match arch {
                "aarch64" => "arm64",
                "x86_64" => "x64",
                _ => return Err(unknown()),
            };
            let (platform, ext) = match (os, variant) {
                ("macos", None) => (format!("macos-{arch}"), "tar.gz"),
                ("macos", Some(v)) => {
                    return Err(format!(
                        "llama.cpp publishes no `{v}` build for macOS: Metal is the one there is"
                    ));
                }
                ("linux", None) => (format!("ubuntu-{arch}"), "tar.gz"),
                ("linux", Some(v)) => (format!("ubuntu-{v}-{arch}"), "tar.gz"),
                ("windows", v) => (format!("win-{}-{arch}", v.unwrap_or("cpu")), "zip"),
                _ => return Err(unknown()),
            };
            Ok(format!("llama-{version}-bin-{platform}.{ext}"))
        }
        EngineKind::Ollama => {
            let suffix = variant.map(|v| format!("-{v}")).unwrap_or_default();
            match (os, arch) {
                ("macos", _) => Ok("ollama-darwin.tgz".into()),
                ("linux", "x86_64") => Ok(format!("ollama-linux-amd64{suffix}.tar.zst")),
                ("linux", "aarch64") => Ok(format!("ollama-linux-arm64{suffix}.tar.zst")),
                ("windows", "x86_64") => Ok("ollama-windows-amd64.zip".into()),
                ("windows", "aarch64") => Ok("ollama-windows-arm64.zip".into()),
                _ => Err(unknown()),
            }
        }
    }
}

// ---- which binary ---------------------------------------------------------

/// `<state dir>/engines`, beside `config.toml`.
pub fn root() -> Option<PathBuf> {
    Some(
        crate::provider::Config::path_for_writing()?
            .parent()?
            .join("engines"),
    )
}

/// Where one managed copy lives: a directory per kind, version and variant, so
/// two variants of one release do not overwrite each other.
pub fn managed_dir(root: &Path, kind: EngineKind, version: &str, variant: Option<&str>) -> PathBuf {
    let name = match variant {
        Some(variant) => format!("{version}-{variant}"),
        None => version.to_string(),
    };
    root.join(kind.as_str()).join(name)
}

/// The binary inside an unpacked release. Searched rather than assumed: the
/// two projects lay their archives out differently, and both have changed it.
pub fn find_in(dir: &Path, name: &str) -> Option<PathBuf> {
    fn walk(dir: &Path, name: &str, depth: usize) -> Option<PathBuf> {
        let entries = std::fs::read_dir(dir).ok()?;
        let mut dirs = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                dirs.push(path);
            } else if path.file_name().is_some_and(|n| n == name) {
                return Some(path);
            }
        }
        if depth == 0 {
            return None;
        }
        dirs.sort();
        dirs.iter().find_map(|d| walk(d, name, depth - 1))
    }
    walk(dir, name, 4)
}

/// Every copy of this kind's binary on the machine: `PATH`, then the places a
/// package manager or an app puts one without adding it there.
pub fn discover(kind: EngineKind) -> Vec<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect())
        .unwrap_or_default();
    dirs.extend(["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin"].map(PathBuf::from));
    if let Some(home) = &home {
        dirs.push(home.join(".local/bin"));
    }
    if kind == EngineKind::Ollama {
        dirs.push("/Applications/Ollama.app/Contents/Resources".into());
    }
    let mut found: Vec<PathBuf> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for dir in dirs {
        let candidate = dir.join(kind.binary_name());
        if !is_executable(&candidate) {
            continue;
        }
        // One entry per real file: Homebrew's `bin/` is a symlink farm, and
        // the same binary reached twice is not two choices.
        let real = std::fs::canonicalize(&candidate).unwrap_or_else(|_| candidate.clone());
        if seen.insert(real) {
            found.push(candidate);
        }
    }
    found
}

fn is_executable(path: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.is_file() && meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        meta.is_file()
    }
}

/// Whether the page may name this path as the binary. Only if it is the
/// kind's own executable by name: an engine is a process luu runs with
/// arguments, and a page that could name *any* executable is a page that runs
/// anything. The file itself may name what it likes — whoever edits it already
/// has a shell.
pub fn accepts_from_page(kind: EngineKind, binary: &str) -> bool {
    binary == "managed"
        || Path::new(binary)
            .file_name()
            .is_some_and(|name| name == kind.binary_name())
}

/// The binary an engine would run, or why there is none.
pub fn resolve_binary(engine: &Engine) -> Result<PathBuf, String> {
    let kind = engine.kind;
    match engine.binary.as_deref() {
        Some("managed") => {
            let version = engine.version.as_deref().unwrap_or(kind.pinned());
            let root = root().ok_or("this machine has no state directory yet")?;
            let dir = managed_dir(&root, kind, version, engine.variant.as_deref());
            find_in(&dir, kind.binary_name()).ok_or_else(|| {
                format!(
                    "luu's own {} {version} is not downloaded yet",
                    kind.as_str()
                )
            })
        }
        Some(path) => {
            let path = crate::provider::expand_home(Path::new(path));
            match is_executable(&path) {
                true => Ok(path),
                false => Err(format!("{} is not an executable file", path.display())),
            }
        }
        None => discover(kind).into_iter().next().ok_or_else(|| {
            format!(
                "no {} on this machine: download luu's own copy, or name a path",
                kind.binary_name()
            )
        }),
    }
}

/// The one directory `dir` holds, when it holds that and nothing else.
fn sole_dir(dir: &Path) -> Option<PathBuf> {
    let mut entries = std::fs::read_dir(dir).ok()?.flatten();
    let only = entries.next()?;
    match entries.next().is_none() && only.file_type().ok()?.is_dir() {
        true => Some(only.path()),
        false => None,
    }
}

/// How far up from a kind's own port luu looks for a free one.
const PORTS_TRIED: u16 = 100;

/// The first port from the kind's own upward that no table names and nothing
/// on this machine listens on — a bind on `127.0.0.1` is the test — within
/// [`PORTS_TRIED`] of it, and under the range the OS hands outgoing
/// connections (32768 and up on Linux, 49152 on macOS), where a port free now
/// is a taken one a minute later. `None` when every one of them is taken.
pub fn free_port(kind: EngineKind, named: &[u16]) -> Option<u16> {
    let from = kind.default_port();
    (from..from.saturating_add(PORTS_TRIED).min(32768))
        .filter(|port| !named.contains(port))
        .find(|port| std::net::TcpListener::bind(("127.0.0.1", *port)).is_ok())
}

// ---- where it listens ------------------------------------------------------

/// The ports this process placed its automatic engines on, by engine name. An
/// `[engine.<name>]` with no `port` is placed the first time anything needs
/// its address — the session that starts it, the page listing its models —
/// on the first free port from its kind's own, and **stays there for the life
/// of the process**, so every backend built for it in the meantime dials the
/// same port. A fact about this process and not about the file, which is why
/// it is not written down. See
/// `RECORD/2026-10-26.ports-luu-places.completed.md`.
static PLACED: std::sync::Mutex<BTreeMap<String, u16>> = std::sync::Mutex::new(BTreeMap::new());

fn placements() -> std::sync::MutexGuard<'static, BTreeMap<String, u16>> {
    PLACED.lock().unwrap_or_else(|e| e.into_inner())
}

/// Where this process placed `name`, if it has.
pub fn placed(name: &str) -> Option<u16> {
    placements().get(name).copied()
}

/// Every port this process placed an engine on.
pub fn placed_ports() -> Vec<u16> {
    placements().values().copied().collect()
}

/// Where `name` listens for this process: where it was placed, or — the first
/// time — the first free port from its kind's own that no table names
/// (`fixed`) and no other automatic engine was placed on. `None` when
/// [`free_port`] finds none.
pub fn place(name: &str, kind: EngineKind, fixed: &[u16]) -> Option<u16> {
    let mut placed = placements();
    if let Some(port) = placed.get(name) {
        return Some(*port);
    }
    let taken: Vec<u16> = fixed.iter().chain(placed.values()).copied().collect();
    let port = free_port(kind, &taken)?;
    placed.insert(name.to_string(), port);
    Some(port)
}

/// Forgets where `name` was placed, so the next [`place`] looks again: what
/// happens when something that is not luu's has taken its port since.
pub fn unplace(name: &str) {
    placements().remove(name);
}

/// The port `name` listens on: its table's, or where this process placed it.
/// `None` for an automatic engine nothing has needed yet.
pub fn port_of(name: &str, engine: &Engine) -> Option<u16> {
    engine.port.or_else(|| placed(name))
}

/// Whether `engine` is an automatic one this process placed at its current
/// port, as opposed to one whose table names the port.
fn is_placed(name: &str, engine: &Engine) -> bool {
    engine.port.is_some() && placed(name) == engine.port
}

/// `url` sent to `port` instead, its scheme, host and path kept: a profile
/// that names an engine sends wherever that engine listens, whatever port
/// its own URL was written with. `None` for a URL that will not parse.
pub fn at_port(url: &str, port: u16) -> Option<String> {
    let mut parsed = reqwest::Url::parse(url).ok()?;
    parsed.set_port(Some(port)).ok()?;
    let text = parsed.to_string();
    // `Url` writes a bare host with a slash after it; a URL that had none
    // keeps having none, so the line a run prints is the one in the file.
    Some(match url.ends_with('/') || !text.ends_with('/') {
        true => text,
        false => text.trim_end_matches('/').to_string(),
    })
}

/// Whether a version and a variant the page sent may be joined into a path
/// under the state directory: a release tag's characters, and not led by a
/// `.`, so neither `..` nor a hidden directory is a name. Install had taken
/// them as they came.
fn tags(version: &str, variant: Option<&str>) -> Result<(), String> {
    let tag = |text: &str| {
        !text.is_empty()
            && !text.starts_with('.')
            && text
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '+'))
    };
    match (tag(version), variant.is_none_or(tag)) {
        (true, true) => Ok(()),
        (false, _) => Err(format!("`{version}` is not a release tag")),
        (_, false) => Err(format!(
            "`{}` is not a build variant",
            variant.unwrap_or("")
        )),
    }
}

/// An engine somebody switched off is not started, by a session or by a click:
/// it says so, where a session sent to it would otherwise fail to connect.
fn in_use(name: &str, engine: &Engine) -> Result<(), String> {
    match engine.enabled {
        true => Ok(()),
        false => Err(format!(
            "engine `{name}` is not in use: switch it on in Settings → Engines"
        )),
    }
}

/// The port a start binds: the table's, where this process placed it, or a
/// place found now.
fn listening(name: &str, engine: &Engine) -> Result<u16, String> {
    port_of(name, engine)
        .or_else(|| place(name, engine.kind, &[]))
        .ok_or_else(|| {
            let from = engine.kind.default_port();
            format!(
                "no free port for engine {name} from {from} to {}",
                from + PORTS_TRIED - 1
            )
        })
}

/// The file `-m` is given: a reference out of [`crate::models`], or a path.
fn model_path(reference: &str, kind: EngineKind) -> Result<PathBuf, String> {
    if !crate::models::is_reference(reference) {
        return Ok(crate::provider::expand_home(Path::new(reference)));
    }
    let model = crate::models::resolve(reference)
        .ok_or_else(|| format!("{reference} is not on this machine"))?;
    model
        .runs_on(kind)
        .map_err(|why| format!("{}: {why}", model.label))?;
    let path = PathBuf::from(&model.path);
    // mlx-serve tells a GGUF from an MLX directory by the extension, and an
    // ollama blob has none: handed `sha256-…` it answers `NotDir`. A link
    // named `.gguf` beside the engines is what it is given instead — nothing
    // is copied. Measured with mlx-serve v26.9.6 on `qwen2.5-coder:7b`.
    if kind == EngineKind::Mlx && model.gguf() && path.extension().is_none_or(|e| e != "gguf") {
        return gguf_link(&model.reference, &path);
    }
    Ok(path)
}

fn gguf_link(reference: &str, target: &Path) -> Result<PathBuf, String> {
    let dir = root()
        .ok_or("this machine has no state directory yet")?
        .join("links");
    std::fs::create_dir_all(&dir).map_err(|e| format!("creating {}: {e}", dir.display()))?;
    let name: String = reference
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let link = dir.join(format!("{name}.gguf"));
    let _ = std::fs::remove_file(&link);
    #[cfg(unix)]
    std::os::unix::fs::symlink(target, &link)
        .map_err(|e| format!("linking {}: {e}", link.display()))?;
    #[cfg(not(unix))]
    std::fs::hard_link(target, &link).map_err(|e| format!("linking {}: {e}", link.display()))?;
    Ok(link)
}

/// The engine a run on this destination needs, **on the model it asked
/// for**: a llama.cpp engine takes the session's model when that is a
/// reference into what this machine has, and its table's own otherwise. That
/// is how choosing a model in the chat moves the engine onto it.
pub fn for_run(resolved: &crate::provider::Resolved) -> Option<(String, Engine)> {
    let (name, mut engine) = resolved.engine.clone()?;
    if engine.kind != EngineKind::Ollama && crate::models::is_reference(&resolved.model) {
        engine.model = Some(resolved.model.clone());
    }
    Some((name, engine))
}

/// A backend on an engine luu starts, which makes sure the engine is up — on
/// the model asked for — before every call it sends.
///
/// This is what lets a switch answer at once: the session is pointed at the
/// new destination straight away, [`Supervisor::warm`] loads it in the
/// background, and a turn sent before it is done waits here rather than
/// meeting a closed port. Once it answers, `ensure` is one health probe.
pub struct Started {
    inner: Arc<dyn agent_core::backend::Backend>,
    supervisor: Arc<Supervisor>,
    name: String,
    engine: Engine,
}

impl Started {
    pub fn new(
        inner: Arc<dyn agent_core::backend::Backend>,
        supervisor: Arc<Supervisor>,
        name: String,
        engine: Engine,
    ) -> Self {
        Self {
            inner,
            supervisor,
            name,
            engine,
        }
    }
}

impl agent_core::backend::Backend for Started {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn stream(
        &self,
        request: agent_core::backend::CompletionRequest,
    ) -> agent_core::backend::ChunkStream<'_> {
        use agent_core::backend::{BackendError, ChunkStream};
        use futures_util::StreamExt;
        Box::pin(
            futures_util::stream::once(async move {
                match self.supervisor.ensure(&self.name, &self.engine).await {
                    Ok(()) => self.inner.stream(request),
                    Err(error) => Box::pin(futures_util::stream::iter([Err(
                        BackendError::Transport(error),
                    )])) as ChunkStream<'_>,
                }
            })
            .flatten(),
        )
    }

    fn models(&self) -> agent_core::backend::BackendFuture<'_, Vec<String>> {
        self.inner.models()
    }

    /// Asked of the engine once it is up — a window read off a server still
    /// loading is no window at all.
    fn window(&self) -> agent_core::backend::BackendFuture<'_, Option<u32>> {
        Box::pin(async move {
            if self
                .supervisor
                .ensure(&self.name, &self.engine)
                .await
                .is_err()
            {
                return Ok(None);
            }
            self.inner.window().await
        })
    }

    fn constrain_caveat(&self, constraint: &agent_core::backend::Constraint) -> Option<String> {
        self.inner.constrain_caveat(constraint)
    }

    fn sampling_caveat(&self, temperature: Option<f32>, seed: Option<u32>) -> Option<String> {
        self.inner.sampling_caveat(temperature, seed)
    }
}

// ---- its life -------------------------------------------------------------

/// What the page is told about one engine.
#[derive(Debug, Clone, Serialize)]
pub struct Status {
    /// `running`, `stopped`, or `exited` — the last with `exit` saying how.
    pub state: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit: Option<String>,
    /// Whether something answers on the port, ours or not: an Ollama app the
    /// person already runs is a server this one does not need to start.
    pub answering: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub binary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_secs: Option<u64>,
    pub log: Vec<String>,
}

struct Running {
    child: tokio::process::Child,
    binary: PathBuf,
    started: Instant,
    /// The reference it was started on, so a session that asks for another
    /// can tell that this one is not it.
    model: Option<String>,
}

#[derive(Default)]
struct Slot {
    running: Option<Running>,
    exit: Option<String>,
    log: Arc<std::sync::Mutex<VecDeque<String>>>,
}

impl Slot {
    /// Reaps a child that exited on its own, so its status is what it did
    /// rather than what it was last seen doing.
    fn reap(&mut self) {
        if let Some(running) = &mut self.running
            && let Ok(Some(status)) = running.child.try_wait()
        {
            self.exit = Some(status.to_string());
            self.running = None;
        }
    }

    fn tail(&self, n: usize) -> Vec<String> {
        let log = self.log.lock().unwrap_or_else(|e| e.into_inner());
        log.iter()
            .skip(log.len().saturating_sub(n))
            .cloned()
            .collect()
    }
}

/// One download, as the page polls it.
#[derive(Debug, Clone, Serialize)]
pub struct Install {
    pub kind: EngineKind,
    pub version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub variant: Option<String>,
    pub asset: String,
    /// `downloading`, `verifying`, `unpacking`, `done` or `failed`.
    pub state: &'static str,
    pub done: u64,
    pub total: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// What it did, a line per step, as the form's *View details* shows it:
    /// the release asked for, the asset and its digest, the download in
    /// tenths, the check, the unpack, and where the copy landed or why not.
    pub log: Vec<String>,
}

/// The engines this server started, and the downloads it is running.
#[derive(Default)]
pub struct Supervisor {
    slots: Mutex<BTreeMap<String, Slot>>,
    installs: std::sync::Mutex<BTreeMap<String, Install>>,
    /// One [`Supervisor::ensure`] at a time per engine. A switch warms the
    /// engine in the background and the first turn waits on the same engine;
    /// without this the two raced to stop and start it, and the loser could
    /// stop the model the winner had just loaded.
    ensuring: std::sync::Mutex<BTreeMap<String, Arc<Mutex<()>>>>,
    /// What the last `ensure` of each engine came to, for the page to read
    /// while it waits instead of waiting inside a request.
    readiness: std::sync::Mutex<BTreeMap<String, Readiness>>,
}

/// Where one engine's last [`Supervisor::ensure`] stands.
#[derive(Debug, Clone)]
struct Readiness {
    state: &'static str,
    model: Option<String>,
    from: Option<String>,
    since: Instant,
    error: Option<String>,
}

/// Whether a phase is news, which is what the log is told.
///
/// `warm` notes `starting` so the page's clock begins at the click, and the
/// `ensure` it spawns notes `starting` again just before the process starts;
/// every model call runs `ensure` and notes `ready` on an engine that already
/// was. Both are the same fact twice, and the log said each one: two `starting`
/// lines at every `serve` start, read as the engine starting twice, and a
/// `ready` per call — 47 of them in a day that started the process 8 times.
/// The readiness table is still written every time; only the line is skipped.
fn moved(
    previous: Option<&Readiness>,
    state: &str,
    model: &Option<String>,
    error: &Option<String>,
) -> bool {
    previous.is_none_or(|was| was.state != state || &was.model != model || &was.error != error)
}

/// The states an `ensure` passes through before it ends, in order. The clock
/// runs across all of them: a person reads *how long since I switched*, not
/// how long the current phase has taken.
const IN_PROGRESS: [&str; 3] = ["stopping", "downloading", "starting"];

/// [`Readiness`] as the page reads it.
#[derive(Debug, Clone, Serialize)]
pub struct ReadinessView {
    /// `stopping` (the engine is being restarted off `from`), `downloading`
    /// (luu's own copy), `starting` (loading `model`), then `ready` or
    /// `failed`.
    pub state: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// The model a `stopping` engine was running.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    /// Since the `ensure` began, across its phases; since the end for a
    /// `ready` or a `failed`.
    pub secs: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

fn http() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent(concat!("luu/", env!("CARGO_PKG_VERSION")))
        .build()
        .unwrap_or_default()
}

async fn answering(kind: EngineKind, port: u16) -> bool {
    let url = format!("http://127.0.0.1:{port}{}", kind.health());
    match http().get(url).timeout(Duration::from_secs(2)).send().await {
        Ok(response) => response.status().is_success(),
        Err(_) => false,
    }
}

fn pipe(
    from: impl AsyncRead + Unpin + Send + 'static,
    log: Arc<std::sync::Mutex<VecDeque<String>>>,
) {
    tokio::spawn(async move {
        let mut lines = BufReader::new(from).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let mut log = log.lock().unwrap_or_else(|e| e.into_inner());
            if log.len() == LOG_LINES {
                log.pop_front();
            }
            log.push_back(line);
        }
    });
}

impl Supervisor {
    pub async fn status(&self, name: &str, engine: &Engine) -> Status {
        // An automatic engine nothing has placed listens nowhere yet, and
        // asking its kind's own port would report whoever else is there.
        let answering = match port_of(name, engine) {
            Some(port) => answering(engine.kind, port).await,
            None => false,
        };
        let mut slots = self.slots.lock().await;
        let slot = slots.entry(name.to_string()).or_default();
        slot.reap();
        Status {
            state: match (&slot.running, &slot.exit) {
                (Some(_), _) => "running",
                (None, Some(_)) => "exited",
                (None, None) => "stopped",
            },
            pid: slot.running.as_ref().and_then(|r| r.child.id()),
            exit: slot.exit.clone(),
            answering,
            binary: slot
                .running
                .as_ref()
                .map(|r| r.binary.display().to_string()),
            started_secs: slot.running.as_ref().map(|r| r.started.elapsed().as_secs()),
            log: slot.tail(40),
        }
    }

    /// Starts it unless this server already has it running.
    pub async fn start(&self, name: &str, engine: &Engine) -> Result<(), String> {
        in_use(name, engine)?;
        let binary = resolve_binary(engine)?;
        let port = listening(name, engine)?;
        let mut slots = self.slots.lock().await;
        let slot = slots.entry(name.to_string()).or_default();
        slot.reap();
        if slot.running.is_some() {
            return Ok(());
        }
        let mut command = tokio::process::Command::new(&binary);
        match engine.kind {
            EngineKind::Llama => {
                command.args(["--host", "127.0.0.1", "--port", &port.to_string()]);
                if let Some(reference) = &engine.model {
                    command.arg("-m").arg(model_path(reference, engine.kind)?);
                }
            }
            EngineKind::Mlx => {
                command.args([
                    "--serve",
                    "--host",
                    "127.0.0.1",
                    "--port",
                    &port.to_string(),
                ]);
                if let Some(reference) = &engine.model {
                    command
                        .arg("--model")
                        .arg(model_path(reference, engine.kind)?);
                }
            }
            EngineKind::Ollama => {
                command
                    .arg("serve")
                    .env("OLLAMA_HOST", format!("127.0.0.1:{port}"));
            }
        }
        command
            .args(&engine.args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            // The second half of "tied to `luu serve`": the first is `stop_all`
            // on a signal, and this is every other way the handle goes away.
            .kill_on_drop(true);
        let mut child = command
            .spawn()
            .map_err(|error| format!("starting {}: {error}", binary.display()))?;
        tracing::info!(target: "luu::engines", engine = name, pid = child.id(), binary = %binary.display(), model = engine.model.as_deref(), "engine process started");
        {
            let mut log = slot.log.lock().unwrap_or_else(|e| e.into_inner());
            log.clear();
            let model = match (&engine.kind, &engine.model) {
                (EngineKind::Llama, Some(reference)) => format!(" -m <{reference}>"),
                (EngineKind::Mlx, Some(reference)) => format!(" --model <{reference}>"),
                _ => String::new(),
            };
            log.push_back(format!(
                "$ {}{model} {}",
                binary.display(),
                engine.args.join(" ")
            ));
        }
        if let Some(out) = child.stdout.take() {
            pipe(out, slot.log.clone());
        }
        if let Some(err) = child.stderr.take() {
            pipe(err, slot.log.clone());
        }
        slot.exit = None;
        slot.running = Some(Running {
            child,
            binary,
            started: Instant::now(),
            model: engine.model.clone(),
        });
        Ok(())
    }

    /// Answering on its port, on the model asked for, within the table's
    /// `ready-timeout`. What a session on a profile with `engine` waits for,
    /// and all of it automatic:
    ///
    /// - luu's own copy is **downloaded** the first time it is needed;
    /// - an engine this server runs on **another model is restarted** on this
    ///   one — llama-server holds one model, and the session asked for this;
    /// - a server something else runs on the port is used as it is: it is not
    ///   luu's to restart.
    ///
    /// One at a time per engine: a second caller waits for the first and then
    /// finds it answering. Its outcome is kept for [`Supervisor::readiness`].
    pub async fn ensure(self: &Arc<Self>, name: &str, engine: &Engine) -> Result<(), String> {
        let gate = self
            .ensuring
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(name.to_string())
            .or_default()
            .clone();
        let _held = gate.lock().await;
        let outcome = self.ensure_now(name, engine).await;
        match &outcome {
            Ok(()) => self.note(name, "ready", engine.model.clone(), None, None),
            Err(error) => self.note(
                name,
                "failed",
                engine.model.clone(),
                None,
                Some(error.clone()),
            ),
        }
        outcome
    }

    /// [`Supervisor::ensure`] in the background: what a session switching to
    /// this engine starts, so the switch answers at once and the page reads
    /// the load from [`Supervisor::readiness`]. The first turn calls `ensure`
    /// itself — see [`Started`] — and waits on this one rather than racing it.
    pub fn warm(self: &Arc<Self>, name: &str, engine: &Engine) {
        self.note(name, "starting", engine.model.clone(), None, None);
        let this = self.clone();
        let (name, engine) = (name.to_string(), engine.clone());
        tokio::spawn(async move {
            if let Err(error) = this.ensure(&name, &engine).await {
                eprintln!("engine {name}: {error}");
            }
        });
    }

    /// Where the last `ensure` of `name` stands, or `None` if nothing has
    /// asked for it since `serve` started.
    pub fn readiness(&self, name: &str) -> Option<ReadinessView> {
        let readiness = self.readiness.lock().unwrap_or_else(|e| e.into_inner());
        readiness.get(name).map(|r| ReadinessView {
            state: r.state,
            model: r.model.clone(),
            from: r.from.clone(),
            secs: r.since.elapsed().as_secs(),
            error: r.error.clone(),
        })
    }

    fn note(
        &self,
        name: &str,
        state: &'static str,
        model: Option<String>,
        from: Option<String>,
        error: Option<String>,
    ) {
        let mut readiness = self.readiness.lock().unwrap_or_else(|e| e.into_inner());
        if moved(readiness.get(name), state, &model, &error) {
            match &error {
                Some(error) => {
                    tracing::warn!(target: "luu::engines", engine = name, state, model = model.as_deref(), from = from.as_deref(), error = error.as_str(), "engine")
                }
                None => {
                    tracing::info!(target: "luu::engines", engine = name, state, model = model.as_deref(), from = from.as_deref(), "engine")
                }
            }
        }
        // One clock per switch: a phase that follows another of the same
        // `ensure` keeps the time it began. `warm` notes `starting` before the
        // task that runs `ensure` gets to, and the page counts from the click.
        let since = match readiness.get(name) {
            Some(r)
                if IN_PROGRESS.contains(&state)
                    && IN_PROGRESS.contains(&r.state)
                    && r.model == model =>
            {
                r.since
            }
            _ => Instant::now(),
        };
        let from = from.or_else(|| {
            readiness
                .get(name)
                .filter(|r| IN_PROGRESS.contains(&state) && r.model == model)
                .and_then(|r| r.from.clone())
        });
        readiness.insert(
            name.to_string(),
            Readiness {
                state,
                model,
                from,
                since,
                error,
            },
        );
    }

    async fn ensure_now(self: &Arc<Self>, name: &str, engine: &Engine) -> Result<(), String> {
        // Before anything is downloaded for it, as well as before it runs.
        in_use(name, engine)?;
        let port = listening(name, engine)?;
        let ours = {
            let mut slots = self.slots.lock().await;
            let slot = slots.entry(name.to_string()).or_default();
            slot.reap();
            slot.running.as_ref().map(|r| r.model.clone())
        };
        match ours {
            Some(model) if model != engine.model && engine.kind != EngineKind::Ollama => {
                self.note(name, "stopping", engine.model.clone(), model, None);
                self.stop(name).await;
            }
            _ => {
                if answering(engine.kind, port).await {
                    // A fixed port that answers is used as it is: it is not
                    // luu's to restart. A port luu *placed* this engine on is
                    // luu's, and something else answering there means it was
                    // taken since — so it is placed again, and the session,
                    // whose backend dials the old port, is asked to start anew.
                    if is_placed(name, engine) {
                        unplace(name);
                        return Err(format!(
                            "port {port}, where luu placed engine {name}, answers and is not \
                             luu's: start a new session to place it on a free one"
                        ));
                    }
                    return Ok(());
                }
            }
        }
        if engine.binary.as_deref() == Some("managed") && resolve_binary(engine).is_err() {
            self.note(name, "downloading", engine.model.clone(), None, None);
            self.install_and_wait(engine).await?;
        }
        self.note(name, "starting", engine.model.clone(), None, None);
        self.start(name, engine).await?;
        let limit = Duration::from_secs(engine.ready_timeout.unwrap_or(DEFAULT_READY_SECS));
        let began = Instant::now();
        loop {
            if answering(engine.kind, port).await {
                return Ok(());
            }
            {
                let mut slots = self.slots.lock().await;
                let slot = slots.entry(name.to_string()).or_default();
                slot.reap();
                if slot.running.is_none() {
                    return Err(format!(
                        "engine {name} exited ({}) before it answered:\n{}",
                        slot.exit.as_deref().unwrap_or("stopped"),
                        slot.tail(8).join("\n")
                    ));
                }
            }
            if began.elapsed() > limit {
                return Err(format!(
                    "engine {name} did not answer on port {port} within {}s — `ready-timeout` \
                     in [engine.{name}] moves the limit",
                    limit.as_secs()
                ));
            }
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
    }

    pub async fn stop(&self, name: &str) {
        let mut slots = self.slots.lock().await;
        if let Some(slot) = slots.get_mut(name)
            && let Some(mut running) = slot.running.take()
        {
            tracing::info!(target: "luu::engines", engine = name, pid = running.child.id(), "engine process stopped");
            let _ = running.child.kill().await;
            slot.exit = Some("stopped from luu".into());
        }
    }

    /// Stops every engine this server started except `keep` — what happens
    /// when a session moves to a destination that does not need them, so an
    /// engine nobody sends to does not hold its model in memory until `serve`
    /// exits. A server luu did not start is never touched.
    pub async fn stop_others(&self, keep: Option<&str>) {
        let mut slots = self.slots.lock().await;
        for (name, slot) in slots.iter_mut() {
            if Some(name.as_str()) == keep {
                continue;
            }
            if let Some(mut running) = slot.running.take() {
                tracing::info!(target: "luu::engines", engine = name.as_str(), pid = running.child.id(), "engine process stopped: no session sends to it");
                let _ = running.child.kill().await;
                slot.exit = Some("stopped: no session sends to it".into());
            }
        }
    }

    /// Every engine this server started. Called when `serve` is asked to stop,
    /// because a signal ends the process without running a single `Drop`.
    pub async fn stop_all(&self) {
        let mut slots = self.slots.lock().await;
        for slot in slots.values_mut() {
            if let Some(mut running) = slot.running.take() {
                let _ = running.child.kill().await;
            }
        }
    }

    // ---- luu's own copy ---------------------------------------------------

    /// Downloads luu's own copy for this engine and waits for it: the
    /// download a session triggers by needing it, shown on the Engines page
    /// like one a person started.
    async fn install_and_wait(self: &Arc<Self>, engine: &Engine) -> Result<(), String> {
        let version = engine
            .version
            .clone()
            .unwrap_or_else(|| engine.kind.pinned().to_string());
        let asset = engine.kind.asset(&version, engine.variant.as_deref())?;
        let key = format!("{}/{}", engine.kind.as_str(), asset);
        self.install(engine.kind, Some(version), engine.variant.clone())?;
        loop {
            let state = self
                .installs
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(&key)
                .map(|i| (i.state, i.error.clone()));
            match state {
                Some(("done", _)) => return Ok(()),
                Some(("failed", error)) => {
                    return Err(error.unwrap_or_else(|| "the download failed".into()));
                }
                None => return Err("the download did not start".into()),
                _ => tokio::time::sleep(Duration::from_millis(300)).await,
            }
        }
    }

    /// Removes luu's own copy of one release: the directory [`managed_dir`]
    /// names, and the download it came from. Refused while it is downloading,
    /// and while an engine this server started runs from inside it — the
    /// slots stay locked until it is gone, so none starts from it meanwhile.
    pub async fn uninstall(
        &self,
        kind: EngineKind,
        version: Option<String>,
        variant: Option<String>,
    ) -> Result<(), String> {
        let root = root().ok_or("this machine has no state directory yet")?;
        let version = version.unwrap_or_else(|| kind.pinned().to_string());
        tags(&version, variant.as_deref())?;
        let dir = managed_dir(&root, kind, &version, variant.as_deref());
        let key = format!(
            "{}/{}",
            kind.as_str(),
            kind.asset(&version, variant.as_deref())?
        );
        if self
            .installs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&key)
            .is_some_and(|i| !matches!(i.state, "done" | "failed"))
        {
            return Err(format!(
                "luu's own {} {version} is still downloading",
                kind.as_str()
            ));
        }
        let mut slots = self.slots.lock().await;
        for (name, slot) in slots.iter_mut() {
            slot.reap();
            if slot
                .running
                .as_ref()
                .is_some_and(|r| r.binary.starts_with(&dir))
            {
                return Err(format!("engine `{name}` is running from it: stop it first"));
            }
        }
        if !dir.is_dir() {
            return Err(format!(
                "luu's own {} {version} is not downloaded",
                kind.as_str()
            ));
        }
        tokio::fs::remove_dir_all(&dir)
            .await
            .map_err(|error| format!("removing {}: {error}", dir.display()))?;
        drop(slots);
        tracing::info!(target: "luu::engines", kind = kind.as_str(), version, dir = %dir.display(), "managed copy removed");
        self.installs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&key);
        Ok(())
    }

    /// The model an engine this server runs was started on: `None` when it
    /// is not running, `Some(None)` when it was started on none.
    pub async fn model_of(&self, name: &str) -> Option<Option<String>> {
        let mut slots = self.slots.lock().await;
        let slot = slots.get_mut(name)?;
        slot.reap();
        slot.running.as_ref().map(|running| running.model.clone())
    }

    pub fn installs(&self) -> Vec<Install> {
        self.installs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .cloned()
            .collect()
    }

    fn update(&self, key: &str, change: impl FnOnce(&mut Install)) {
        if let Some(install) = self
            .installs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get_mut(key)
        {
            change(install);
        }
    }

    /// Starts a download in the background and returns at once; the page
    /// polls [`Supervisor::installs`]. One at a time per copy.
    pub fn install(
        self: &Arc<Self>,
        kind: EngineKind,
        version: Option<String>,
        variant: Option<String>,
    ) -> Result<(), String> {
        let root = root().ok_or("this machine has no state directory yet")?;
        let version = version.unwrap_or_else(|| kind.pinned().to_string());
        tags(&version, variant.as_deref())?;
        let asset = kind.asset(&version, variant.as_deref())?;
        let key = format!("{}/{}", kind.as_str(), asset);
        {
            let mut installs = self.installs.lock().unwrap_or_else(|e| e.into_inner());
            if installs
                .get(&key)
                .is_some_and(|i| !matches!(i.state, "done" | "failed"))
            {
                return Ok(());
            }
            installs.insert(
                key.clone(),
                Install {
                    kind,
                    version: version.clone(),
                    variant: variant.clone(),
                    asset: asset.clone(),
                    state: "downloading",
                    done: 0,
                    total: 0,
                    error: None,
                    log: Vec::new(),
                },
            );
        }
        let this = self.clone();
        tokio::spawn(async move {
            let dir = managed_dir(&root, kind, &version, variant.as_deref());
            this.say(
                &key,
                format!(
                    "$ install {} {version}{} into {}",
                    kind.as_str(),
                    variant
                        .as_deref()
                        .map(|v| format!(" ({v})"))
                        .unwrap_or_default(),
                    dir.display()
                ),
            );
            let result = this.download(&key, kind, &version, &asset, &dir).await;
            this.update(&key, |install| match result {
                Ok(()) => {
                    install.state = "done";
                    install.log.push("done".into());
                }
                Err(error) => {
                    install.state = "failed";
                    install.log.push(format!("failed: {error}"));
                    install.error = Some(error);
                }
            });
        });
        Ok(())
    }

    /// One line of an install's log.
    fn say(&self, key: &str, line: String) {
        self.update(key, |install| install.log.push(line));
    }

    async fn download(
        &self,
        key: &str,
        kind: EngineKind,
        version: &str,
        asset: &str,
        dir: &Path,
    ) -> Result<(), String> {
        use futures_util::StreamExt;
        use sha2::Digest;
        use tokio::io::AsyncWriteExt;

        #[derive(serde::Deserialize)]
        struct Release {
            assets: Vec<Asset>,
        }
        #[derive(serde::Deserialize)]
        struct Asset {
            name: String,
            browser_download_url: String,
            size: u64,
            digest: Option<String>,
        }

        let client = http();
        self.say(key, format!("asking GitHub for {} {version}", kind.repo()));
        let release: Release = client
            .get(format!(
                "https://api.github.com/repos/{}/releases/tags/{version}",
                kind.repo()
            ))
            .header("accept", "application/vnd.github+json")
            .send()
            .await
            .and_then(|r| r.error_for_status())
            .map_err(|e| format!("asking GitHub for {} {version}: {e}", kind.repo()))?
            .json()
            .await
            .map_err(|e| format!("reading GitHub's answer: {e}"))?;
        let found = release
            .assets
            .into_iter()
            .find(|a| a.name == asset)
            .ok_or_else(|| format!("{} {version} has no asset named {asset}", kind.repo()))?;
        // No digest, no download: a file that cannot be checked is a file this
        // would be executing on the release page's word alone.
        let expected = found
            .digest
            .as_deref()
            .and_then(|d| d.strip_prefix("sha256:"))
            .ok_or("GitHub publishes no sha256 for this asset, so it cannot be checked")?
            .to_ascii_lowercase();
        self.update(key, |i| i.total = found.size);
        self.say(
            key,
            format!(
                "{asset} · {:.1} MB · sha256 {}",
                found.size as f64 / 1e6,
                expected
            ),
        );

        let parent = dir.parent().ok_or("no parent directory")?;
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| format!("creating {}: {e}", parent.display()))?;
        let part = parent.join(format!("{asset}.part"));
        let mut file = tokio::fs::File::create(&part)
            .await
            .map_err(|e| format!("creating {}: {e}", part.display()))?;
        let mut hasher = sha2::Sha256::new();
        self.say(key, format!("downloading {}", found.browser_download_url));
        let mut stream = client
            .get(&found.browser_download_url)
            .send()
            .await
            .and_then(|r| r.error_for_status())
            .map_err(|e| format!("downloading {asset}: {e}"))?
            .bytes_stream();
        let mut done = 0u64;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| format!("downloading {asset}: {e}"))?;
            hasher.update(&chunk);
            file.write_all(&chunk)
                .await
                .map_err(|e| format!("writing {}: {e}", part.display()))?;
            let before = done;
            done += chunk.len() as u64;
            self.update(key, |i| i.done = done);
            // A line a tenth, not a line a chunk.
            if found.size > 0 && done * 10 / found.size > before * 10 / found.size {
                self.say(
                    key,
                    format!(
                        "  {:.1} of {:.1} MB · {}%",
                        done as f64 / 1e6,
                        found.size as f64 / 1e6,
                        done * 100 / found.size
                    ),
                );
            }
        }
        file.flush().await.map_err(|e| e.to_string())?;
        drop(file);

        self.update(key, |i| i.state = "verifying");
        self.say(key, "verifying its sha256 against GitHub's".into());
        let actual: String = hasher
            .finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        if actual != expected {
            let _ = tokio::fs::remove_file(&part).await;
            return Err(format!(
                "{asset} does not match the digest GitHub publishes for it \
                 ({actual} ≠ {expected}); deleted, not unpacked"
            ));
        }

        self.say(key, "sha256 matches".into());
        // Unpacked beside and renamed into place, so a failed unpack never
        // leaves half a copy where `resolve_binary` would find it.
        self.update(key, |i| i.state = "unpacking");
        self.say(key, format!("$ tar -xf {}", part.display()));
        let staging = parent.join(format!(
            ".{}.unpack",
            dir.file_name().and_then(|n| n.to_str()).unwrap_or("copy")
        ));
        let _ = tokio::fs::remove_dir_all(&staging).await;
        tokio::fs::create_dir_all(&staging)
            .await
            .map_err(|e| format!("creating {}: {e}", staging.display()))?;
        let output = tokio::process::Command::new("tar")
            .arg("-xf")
            .arg(&part)
            .arg("-C")
            .arg(&staging)
            .output()
            .await
            .map_err(|e| format!("running tar: {e}"))?;
        let _ = tokio::fs::remove_file(&part).await;
        if !output.status.success() {
            let _ = tokio::fs::remove_dir_all(&staging).await;
            let why = String::from_utf8_lossy(&output.stderr);
            let hint = match asset.ends_with(".zst") {
                true => " — a .tar.zst needs `zstd` installed",
                false => "",
            };
            return Err(format!("unpacking {asset}: {}{hint}", why.trim()));
        }
        if find_in(&staging, kind.binary_name()).is_none() {
            let _ = tokio::fs::remove_dir_all(&staging).await;
            return Err(format!(
                "{asset} unpacked, and there is no {} in it",
                kind.binary_name()
            ));
        }
        let _ = tokio::fs::remove_dir_all(dir).await;
        // An archive with one folder at its top — llama.cpp's `llama-b11236/`
        // — is moved in without it: the version is already the directory's
        // name, and a second one under it was only the archive's.
        let top = sole_dir(&staging);
        tokio::fs::rename(top.as_deref().unwrap_or(&staging), dir)
            .await
            .map_err(|e| format!("moving the copy into {}: {e}", dir.display()))?;
        if top.is_some() {
            let _ = tokio::fs::remove_dir_all(&staging).await;
        }
        if let Some(binary) = find_in(dir, kind.binary_name()) {
            self.say(key, format!("installed {}", binary.display()));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An automatic engine is placed once per process — past every port a
    /// table names and every other placement — and stays there, so every
    /// backend built for it dials the same port.
    #[test]
    fn an_automatic_engine_is_placed_once_and_stays() {
        let kind = EngineKind::Llama;
        let first = place("test-placed-a", kind, &[]).expect("a free port");
        assert_eq!(placed("test-placed-a"), Some(first));
        assert_eq!(place("test-placed-a", kind, &[]), Some(first), "it stays");
        let other = place("test-placed-b", kind, &[first + 1]).expect("another");
        assert_ne!(other, first, "not where another automatic engine is");
        assert_ne!(other, first + 1, "not where a table says one listens");
        assert!(placed_ports().contains(&first) && placed_ports().contains(&other));
        unplace("test-placed-a");
        assert_eq!(placed("test-placed-a"), None);
        unplace("test-placed-b");
    }

    /// A profile's URL keeps its path and takes its engine's port.
    #[test]
    fn a_url_moves_to_another_port_with_its_path() {
        assert_eq!(
            at_port("http://127.0.0.1:8080/v1", 8081).as_deref(),
            Some("http://127.0.0.1:8081/v1")
        );
        assert_eq!(
            at_port("http://127.0.0.1:11434", 11435).as_deref(),
            Some("http://127.0.0.1:11435")
        );
        assert_eq!(
            at_port("http://127.0.0.1:8080/", 9000).as_deref(),
            Some("http://127.0.0.1:9000/")
        );
        assert_eq!(at_port("not a url", 9000), None);
    }

    /// A port is free when no table names it and nothing listens on it, and
    /// the search goes up from the kind's own.
    #[test]
    fn a_free_port_is_one_nobody_has() {
        let kind = EngineKind::Llama;
        let first = free_port(kind, &[]).expect("a free port near 8080");
        assert!(first >= kind.default_port() && first < 32768);
        let held = std::net::TcpListener::bind(("127.0.0.1", first)).unwrap();
        let next = free_port(kind, &[]).expect("another");
        assert!(next > first, "{next} after {first}, which is listened on");
        assert_ne!(free_port(kind, &[next]), Some(next), "a table names it");
        drop(held);
    }

    /// An archive with one folder at its top is that folder; one with more
    /// than that, or a file beside it, is itself.
    #[test]
    fn an_archive_s_one_folder_is_not_kept() {
        let dir = std::env::temp_dir().join(format!("luu-sole-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("llama-b1")).unwrap();
        assert_eq!(sole_dir(&dir), Some(dir.join("llama-b1")));
        std::fs::write(dir.join("LICENSE"), "").unwrap();
        assert_eq!(sole_dir(&dir), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A tag the page sends is joined into a path under the state directory,
    /// so it is a name or it is refused — for install and uninstall alike.
    #[test]
    fn a_tag_from_the_page_is_a_name_and_not_a_path() {
        assert!(tags("b11236", None).is_ok());
        assert!(tags("v0.34.4", Some("cuda-12.8")).is_ok());
        for (version, variant) in [
            ("../../etc", None),
            ("b1", Some("..")),
            ("", None),
            ("b1/../x", None),
            (".hidden", None),
        ] {
            assert!(tags(version, variant).is_err(), "{version} {variant:?}");
        }
    }

    /// Switched off is not started, by a click or by a session, and says why
    /// before anything is resolved or downloaded for it.
    #[tokio::test]
    async fn an_engine_switched_off_is_not_started() {
        let supervisor = Arc::new(Supervisor::default());
        let engine = Engine {
            kind: EngineKind::Llama,
            binary: Some("managed".into()),
            enabled: false,
            ..Default::default()
        };
        for error in [
            supervisor.start("off", &engine).await.unwrap_err(),
            supervisor.ensure("off", &engine).await.unwrap_err(),
        ] {
            assert!(error.contains("`off` is not in use"), "{error}");
        }
        assert!(
            supervisor.installs().is_empty(),
            "nothing was downloaded for it"
        );
    }

    /// The names the two projects publish, checked against the release pages
    /// on 2026-09-28 — not derived from anything, so this is the test that says
    /// when one of them renames its archives.
    #[test]
    fn each_platform_gets_the_asset_its_project_publishes() {
        let llama = |os, arch, variant| asset_for(EngineKind::Llama, "b11236", variant, os, arch);
        assert_eq!(
            llama("macos", "aarch64", None).unwrap(),
            "llama-b11236-bin-macos-arm64.tar.gz"
        );
        assert_eq!(
            llama("linux", "x86_64", None).unwrap(),
            "llama-b11236-bin-ubuntu-x64.tar.gz"
        );
        assert_eq!(
            llama("linux", "x86_64", Some("vulkan")).unwrap(),
            "llama-b11236-bin-ubuntu-vulkan-x64.tar.gz"
        );
        assert_eq!(
            llama("windows", "x86_64", None).unwrap(),
            "llama-b11236-bin-win-cpu-x64.zip"
        );
        assert!(llama("macos", "aarch64", Some("cuda-12.8")).is_err());

        let mlx = |os, arch| asset_for(EngineKind::Mlx, "v26.9.6", None, os, arch);
        assert_eq!(
            mlx("macos", "aarch64").unwrap(),
            "mlx-serve-bin-macos-arm64.tar.gz"
        );
        assert!(mlx("linux", "x86_64").is_err(), "MLX is Apple Silicon's");

        let ollama =
            |os, arch, variant| asset_for(EngineKind::Ollama, "v0.34.4", variant, os, arch);
        assert_eq!(
            ollama("macos", "aarch64", None).unwrap(),
            "ollama-darwin.tgz"
        );
        assert_eq!(
            ollama("linux", "x86_64", None).unwrap(),
            "ollama-linux-amd64.tar.zst"
        );
        assert_eq!(
            ollama("linux", "x86_64", Some("rocm")).unwrap(),
            "ollama-linux-amd64-rocm.tar.zst"
        );
    }

    /// The session's model moves a llama.cpp engine when it is a reference
    /// into this machine's models, and never an ollama one, which serves them
    /// all.
    #[test]
    fn a_session_picks_the_model_its_llama_engine_loads() {
        let text = "[engine.local]\nkind = \"llama\"\nmodel = \"ollama:a:1b\"\n\n\
                    [engine.ol]\nkind = \"ollama\"\n\n\
                    [provider.local]\nbackend = \"openai\"\nengine = \"local\"\n\n\
                    [provider.ol]\nbackend = \"ollama\"\nengine = \"ol\"\n";
        let config = crate::provider::Config::from_toml(text, "config.toml").unwrap();
        let resolve = |provider, model| {
            crate::provider::resolve(
                &config,
                Some(Path::new("config.toml")),
                crate::provider::Flags {
                    provider: Some(provider),
                    model,
                    ..Default::default()
                },
            )
            .unwrap()
        };

        // Nothing asked: the table's own, and the session says so.
        let own = resolve("local", None);
        assert_eq!(own.model, "ollama:a:1b");
        assert_eq!(
            for_run(&own).unwrap().1.model.as_deref(),
            Some("ollama:a:1b")
        );
        // Another reference: the engine moves onto it.
        let other = resolve("local", Some("huggingface:o/r/f.gguf"));
        assert_eq!(
            for_run(&other).unwrap().1.model.as_deref(),
            Some("huggingface:o/r/f.gguf")
        );
        // A plain name is the server's business, not a file to load.
        let plain = resolve("local", Some("whatever"));
        assert_eq!(
            for_run(&plain).unwrap().1.model.as_deref(),
            Some("ollama:a:1b")
        );
        // ollama serves every model it has, so its engine never moves.
        let ol = resolve("ol", Some("ollama:a:1b"));
        assert_eq!(for_run(&ol).unwrap().1.model, None);
    }

    #[test]
    fn the_page_may_name_only_the_kinds_own_binary() {
        assert!(accepts_from_page(EngineKind::Llama, "managed"));
        assert!(accepts_from_page(
            EngineKind::Llama,
            "/opt/homebrew/bin/llama-server"
        ));
        assert!(accepts_from_page(EngineKind::Ollama, "~/bin/ollama"));
        assert!(!accepts_from_page(EngineKind::Llama, "/bin/sh"));
        assert!(!accepts_from_page(
            EngineKind::Ollama,
            "/usr/bin/ollama-but-not"
        ));
        assert!(!accepts_from_page(
            EngineKind::Llama,
            "/opt/homebrew/bin/ollama"
        ));
    }

    #[test]
    fn a_managed_copy_is_found_wherever_the_archive_put_it() {
        let root = std::env::temp_dir().join(format!("luu-engines-{}", std::process::id()));
        let dir = managed_dir(&root, EngineKind::Llama, "b1", None);
        std::fs::create_dir_all(dir.join("build/bin")).unwrap();
        std::fs::write(dir.join("build/bin/llama-server"), "").unwrap();
        assert_eq!(
            find_in(&dir, "llama-server"),
            Some(dir.join("build/bin/llama-server"))
        );
        assert_eq!(
            managed_dir(&root, EngineKind::Llama, "b1", Some("vulkan")),
            root.join("llama/b1-vulkan")
        );
        std::fs::remove_dir_all(&root).ok();
    }

    /// A switch warms the engine and answers; the turn that follows waits on
    /// the same `ensure` inside the backend, and an engine that cannot start
    /// reaches it as a transport error rather than as a request that hung.
    /// Port 9 on loopback: nothing answers the discard port on a dev machine.
    #[tokio::test]
    async fn a_warm_engine_is_waited_on_by_the_backend_and_says_why_it_failed() {
        use agent_core::backend::{Backend, CompletionRequest};
        use futures_util::StreamExt;

        let supervisor = Arc::new(Supervisor::default());
        let engine = Engine {
            kind: EngineKind::Llama,
            binary: Some("/nonexistent/llama-server".into()),
            port: Some(9),
            model: Some("ollama:qwen2.5-coder:7b".into()),
            ..Default::default()
        };
        supervisor.warm("gone", &engine);
        // Loading from the moment of the click, before the task runs.
        assert_eq!(supervisor.readiness("gone").unwrap().state, "starting");

        let backend = Started::new(
            Arc::new(agent_core::backend::mock::Mock::default()),
            supervisor.clone(),
            "gone".into(),
            engine,
        );
        let first = backend
            .stream(CompletionRequest {
                tools: Vec::new(),
                model: "mock".into(),
                messages: Vec::new(),
                context_limit: None,
                temperature: None,
                seed: None,
                constraint: None,
            })
            .next()
            .await;
        let error = match first {
            Some(Err(error)) => error.to_string(),
            other => panic!("expected the engine's failure, got {other:?}"),
        };
        assert!(error.contains("not an executable file"), "{error}");

        let readiness = supervisor.readiness("gone").unwrap();
        assert_eq!(readiness.state, "failed");
        assert!(readiness.error.unwrap().contains("not an executable file"));
        assert!(supervisor.readiness("never-asked").is_none());
    }

    /// A restart reads as one switch: `stopping` names what it was running,
    /// `starting` still names it, and the clock does not restart between them.
    /// A new switch — another model — starts a clock of its own.
    #[test]
    fn the_phases_of_one_switch_share_a_clock_and_remember_what_was_stopped() {
        let supervisor = Supervisor::default();
        let to = Some("ollama:gemma3:12b".to_string());
        supervisor.note(
            "e",
            "stopping",
            to.clone(),
            Some("ollama:qwen2.5-coder:7b".into()),
            None,
        );
        std::thread::sleep(Duration::from_millis(1100));
        supervisor.note("e", "starting", to.clone(), None, None);
        let starting = supervisor.readiness("e").unwrap();
        assert_eq!(starting.state, "starting");
        assert_eq!(starting.from.as_deref(), Some("ollama:qwen2.5-coder:7b"));
        assert_eq!(starting.secs, 1);

        supervisor.note("e", "ready", to, None, None);
        let ready = supervisor.readiness("e").unwrap();
        assert_eq!((ready.state, ready.from, ready.secs), ("ready", None, 0));

        supervisor.note("e", "starting", Some("other".into()), None, None);
        assert_eq!(supervisor.readiness("e").unwrap().secs, 0);
    }

    #[test]
    fn only_a_phase_that_changes_something_is_news() {
        let model = Some("ollama:qwen2.5-coder:7b".to_string());
        let was = |state, error: Option<&str>| Readiness {
            state,
            model: model.clone(),
            from: None,
            since: Instant::now(),
            error: error.map(str::to_string),
        };
        // The first word about an engine always is.
        assert!(moved(None, "starting", &model, &None));
        // `warm`'s `starting`, then `ensure`'s: once.
        assert!(!moved(
            Some(&was("starting", None)),
            "starting",
            &model,
            &None
        ));
        // A call that finds it answering: silent.
        assert!(!moved(Some(&was("ready", None)), "ready", &model, &None));
        assert!(moved(Some(&was("starting", None)), "ready", &model, &None));
        // Another model, or another reason to have failed, is news.
        assert!(moved(
            Some(&was("starting", None)),
            "starting",
            &Some("other".into()),
            &None
        ));
        assert!(moved(
            Some(&was("failed", Some("port busy"))),
            "failed",
            &model,
            &Some("exited".into())
        ));
    }
}
