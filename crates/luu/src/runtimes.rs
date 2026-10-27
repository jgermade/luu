//! What Settings → Runtimes shows and does: whether each container runtime is
//! installed, answers, and has the images the postures name; the tasks that
//! change that — install where Homebrew can without root, start, build an
//! image — and which policy files in the base a posture may name. And the one
//! thing a session asks of it: start a runtime that is down.
//!
//! Every line run here is one `agent_core::worker::Runtime` built — this
//! module runs them and reads what they say, and knows no runtime's API. See
//! `RECORD/2026-10-01.runtimes-in-settings.completed.md` and
//! `RECORD/2026-10-27.runtimes-without-the-wait.completed.md`.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use agent_core::worker::{Hello, Response, Runtime};
use serde::Serialize;
use tokio::io::{AsyncBufReadExt, BufReader};

/// The label luu puts on what it builds, saying from what: `release` or
/// `source`. Only an image that says `release` is rebuilt by itself. See
/// `RECORD/2026-10-27.an-image-that-knows-its-luu.completed.md`.
const FROM_LABEL: &str = "luu.from";

/// How long a probe may take before it is an answer of its own. Docker
/// Desktop's VM takes a while to wake; a runtime that is still silent after
/// this is one a session could not start on either.
const PROBE: Duration = Duration::from_secs(5);

/// The last lines of a build kept for the page: enough to read why it failed.
const TAIL: usize = 40;

/// How long a session waits for a runtime it started to answer. `open -a
/// Docker` returns long before the daemon does, and a VM boots in seconds to
/// tens of seconds; past this it is not coming up for this session.
const WAKING: Duration = Duration::from_secs(120);

/// The image built from a release rather than from source: `Containerfile`'s
/// final stage over the release's own Linux binary. Carried in the binary,
/// because the base it would be read from is usually not luu's checkout. See
/// `RECORD/2026-10-27.runtimes-without-the-wait.completed.md`.
const RELEASE_CONTAINERFILE: &str = include_str!("../../../Containerfile.release");

/// One container runtime, as the section shows it.
#[derive(Debug, Clone, Serialize)]
pub struct RuntimeView {
    pub runtime: String,
    /// Where its program is, or `None` when it is not installed.
    pub path: Option<String>,
    pub hint: Option<&'static str>,
    /// Whether it answered its probe. `None` where it is not installed, so
    /// nothing was asked.
    pub answers: Option<bool>,
    /// The first line it said when it did not answer.
    pub detail: Option<String>,
    /// What this runtime cannot express, where it runs anyway.
    pub gap: Option<&'static str>,
    pub images: Vec<ImageView>,
    /// The line that would install it from the page.
    pub install: Option<String>,
    /// Why the page cannot install it, where it cannot.
    pub install_refused: Option<&'static str>,
    /// Whether the page may start it: installed, not answering, and a line
    /// to start it with.
    pub startable: bool,
}

// ---- installing and starting --------------------------------------------------

/// How the page installs a runtime: Homebrew, on macOS, for the two whose
/// formula needs no administrator. `brew` refuses to run as root wherever it
/// is installed, so this is a process with the person's own authority — the
/// same one an engine's download has. See
/// `RECORD/2026-10-01.runtimes-in-settings.completed.md`.
pub fn install_steps(runtime: Runtime) -> Result<Vec<Vec<String>>, &'static str> {
    let formula = match runtime {
        Runtime::Podman => "podman",
        Runtime::Colima => "colima",
        Runtime::Docker => {
            return Err("not from here: its privileged helper needs an administrator");
        }
        Runtime::Container => {
            return Err("not from here: its package needs an administrator");
        }
        Runtime::Nerdctl => {
            return Err("not from here: it needs root");
        }
        Runtime::Host | Runtime::Direct => return Err("there is nothing to install"),
    };
    if !cfg!(target_os = "macos") {
        return Err("here the package manager needs root: install it from a terminal");
    }
    let Some(brew) = agent_core::worker::which("brew") else {
        return Err("Homebrew is not on luu serve's PATH");
    };
    Ok(vec![vec![
        brew.display().to_string(),
        "install".into(),
        formula.into(),
    ]])
}

/// How the page starts a runtime that is installed and not answering: the
/// daemon, the VM or the service, by each one's own word. `None` where that
/// takes root.
pub fn start_steps(runtime: Runtime) -> Option<Vec<Vec<String>>> {
    let line = |words: &[&str]| words.iter().map(|w| w.to_string()).collect::<Vec<_>>();
    match runtime {
        // The app, which starts its own VM; the daemon answers some seconds
        // after this returns.
        Runtime::Docker if cfg!(target_os = "macos") => Some(vec![line(&["open", "-a", "Docker"])]),
        // A machine is made once and started each time; `init` on one that
        // exists is an error, so it is asked first.
        Runtime::Podman if cfg!(target_os = "macos") => Some(vec![line(&[
            "sh",
            "-c",
            "podman machine inspect >/dev/null 2>&1 || podman machine init; podman machine start",
        ])]),
        Runtime::Colima => Some(vec![line(&["colima", "start", "--runtime", "containerd"])]),
        Runtime::Container => Some(vec![line(&["container", "system", "start"])]),
        _ => None,
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ImageView {
    pub image: String,
    /// `None` where it could not be asked: not installed, or not answering.
    pub present: Option<bool>,
    /// How luu built it, by its `luu.from` label: `release` or `source`.
    /// `None` for an image luu did not label — by hand, or before it did.
    pub from: Option<String>,
    /// Which luu its worker said it is, `0.1.4 (3f2a9c1b)`, where it was
    /// asked.
    pub worker: Option<String>,
    /// How that is not this luu, where it is not: [`Hello::differs`].
    pub stale: Option<String>,
    /// Which build would make it this luu's worker, where it is not: the
    /// release when the version is what moved, and only source when it is
    /// this same version from other sources — a rebuild from the release
    /// would bring the same release back.
    pub cure: Option<BuildFrom>,
    /// Why its worker could not be asked, where it could not.
    pub unasked: Option<String>,
}

/// Every container runtime as far as it can be seen **without asking it**:
/// installed or not, and how to install it. Nothing is run, so it answers at
/// once, and [`probe`] asks each one on its own — the page fills each card
/// when its runtime answers rather than when the slowest one does.
pub fn seen_all(images: &[String]) -> Vec<RuntimeView> {
    Runtime::CONTAINED
        .iter()
        .map(|&runtime| seen(runtime, images))
        .collect()
}

fn seen(runtime: Runtime, images: &[String]) -> RuntimeView {
    let program = runtime.program().unwrap_or_default();
    let path = agent_core::worker::which(program).map(|path| path.display().to_string());
    RuntimeView {
        runtime: runtime.as_str().to_string(),
        path,
        hint: runtime.install_hint(),
        answers: None,
        detail: None,
        gap: runtime.cannot(),
        install: install_steps(runtime).ok().map(|steps| {
            steps
                .iter()
                .map(|step| step.join(" "))
                .collect::<Vec<_>>()
                .join(" && ")
        }),
        install_refused: install_steps(runtime).err(),
        startable: false,
        images: images
            .iter()
            .map(|image| ImageView {
                image: image.clone(),
                present: None,
                from: None,
                worker: None,
                stale: None,
                cure: None,
                unasked: None,
            })
            .collect(),
    }
}

/// One runtime, asked: whether it answers, and whether its store has each of
/// `images`. Not installed is said without running anything.
pub async fn probe(runtime: Runtime, images: &[String]) -> RuntimeView {
    let mut view = seen(runtime, images);
    if view.path.is_none() {
        return view;
    }
    let Some(argv) = runtime.probe_argv() else {
        return view;
    };
    match run(&argv).await {
        Ok(_) => view.answers = Some(true),
        Err(said) => {
            view.answers = Some(false);
            view.detail = Some(said);
            view.startable = start_steps(runtime).is_some();
            return view;
        }
    }
    for image in &mut view.images {
        let Some(argv) = runtime.image_argv(&image.image) else {
            continue;
        };
        let Ok(inspected) = run(&argv).await else {
            image.present = Some(false);
            continue;
        };
        image.present = Some(true);
        image.from = label(&inspected, FROM_LABEL);
        // `built` means *this luu's worker*, not *a tag by that name*: the
        // worker in it is asked, and says.
        match hello_of(runtime, &image.image).await {
            Ok(hello) => {
                image.worker = Some(hello.identity());
                image.stale = hello.differs();
                image.cure = image.stale.as_ref().map(|_| {
                    match hello.version == env!("CARGO_PKG_VERSION") {
                        true => BuildFrom::Source,
                        false => BuildFrom::Release,
                    }
                });
            }
            Err(why) => image.unasked = Some(why),
        }
    }
    view
}

/// How long the worker in an image may take to say its hello: a container
/// start, which on a VM is a second or more.
const HELLO: Duration = Duration::from_secs(20);

/// What the worker in `image` says it is, from a container started with
/// nothing to do: the first line it writes is its hello, and then its input
/// is closed and it exits.
pub async fn hello_of(runtime: Runtime, image: &str) -> Result<Hello, String> {
    let argv = runtime
        .hello_argv(image)
        .ok_or_else(|| format!("`{runtime}` runs no images"))?;
    let said = run_for(&argv, HELLO).await?;
    let line = said
        .lines()
        .find(|line| !line.trim().is_empty())
        .ok_or("its worker said nothing")?;
    match serde_json::from_str::<Response>(line) {
        Ok(Response::Hello(hello)) => Ok(hello),
        Ok(other) => Err(format!(
            "its worker answered something else first: {other:?}"
        )),
        Err(error) => Err(format!("its worker's first line is not a hello: {error}")),
    }
}

/// How luu built `image`, by its `luu.from` label, or `None` where it did not
/// label it or the image is not there.
pub async fn built_from(runtime: Runtime, image: &str) -> Option<String> {
    let argv = runtime.image_argv(image)?;
    label(&run(&argv).await.ok()?, FROM_LABEL)
}

/// A label's value out of `image inspect`'s JSON, wherever the runtime put
/// it: Docker under `Config.Labels`, Podman there and at the top, and the
/// others in their own shapes. Looked for by key rather than by path, since
/// the five do not agree on the path.
fn label(inspected: &str, key: &str) -> Option<String> {
    fn find(value: &serde_json::Value, key: &str) -> Option<String> {
        match value {
            serde_json::Value::Object(map) => map
                .get(key)
                .and_then(|found| found.as_str().map(str::to_string))
                .or_else(|| map.values().find_map(|value| find(value, key))),
            serde_json::Value::Array(items) => items.iter().find_map(|value| find(value, key)),
            _ => None,
        }
    }
    find(&serde_json::from_str(inspected).ok()?, key)
}

/// Runs one line under [`PROBE`]: what it printed, or its first line of
/// complaint when it fails.
async fn run(argv: &[String]) -> Result<String, String> {
    run_for(argv, PROBE).await
}

async fn run_for(argv: &[String], clock: Duration) -> Result<String, String> {
    let mut command = tokio::process::Command::new(&argv[0]);
    command
        .args(&argv[1..])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    let output = match tokio::time::timeout(clock, command.output()).await {
        Err(_) => return Err(format!("did not answer in {}s", clock.as_secs())),
        Ok(Err(error)) => return Err(error.to_string()),
        Ok(Ok(output)) => output,
    };
    match output.status.success() {
        true => Ok(String::from_utf8_lossy(&output.stdout).into_owned()),
        false => Err(String::from_utf8_lossy(&output.stderr)
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .unwrap_or("it exited with an error and said nothing")
            .to_string()),
    }
}

// ---- tasks ------------------------------------------------------------------

/// Where an image's luu comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum BuildFrom {
    /// This version's release, for Linux: no compiler, no checkout.
    #[default]
    Release,
    /// `<base>/Containerfile`, which compiles the checkout inside the image.
    Source,
}

/// Whether `base` is luu's own checkout, the one place a worker is built from
/// source. A `Containerfile` alone is not enough: a person's project has its
/// own, and building *that* under the worker's tag would put their image
/// where luu's worker is expected. So the crate the worker is compiled from has
/// to be there beside it.
pub fn is_luu_checkout(base: &Path) -> bool {
    base.join("Containerfile").is_file() && base.join("crates/agent-core/Cargo.toml").is_file()
}

/// The line that builds `image`, run in the base for a source build and in
/// [`release_context`] for a release one.
fn build_line(runtime: Runtime, image: &str, from: BuildFrom) -> Option<Vec<String>> {
    match from {
        BuildFrom::Source => {
            runtime.build_argv(image, "Containerfile", ".", &[], &[(FROM_LABEL, "source")])
        }
        BuildFrom::Release => runtime.build_argv(
            image,
            "Containerfile.release",
            ".",
            &[("LUU_VERSION", env!("CARGO_PKG_VERSION"))],
            &[(FROM_LABEL, "release")],
        ),
    }
}

/// How long a rebuild from the release may take when a session waits on it:
/// a download of the binary and, the first time, the base image.
const REBUILDING: Duration = Duration::from_secs(600);

/// Rebuilds `image` from this version's release, for a session that is
/// waiting on it: the same line *build* runs in Settings, every line it
/// prints to `say`. See
/// `RECORD/2026-10-27.an-image-that-knows-its-luu.completed.md`.
pub async fn rebuild(runtime: Runtime, image: &str, say: impl Fn(String)) -> Result<(), String> {
    let argv = build_line(runtime, image, BuildFrom::Release)
        .ok_or_else(|| format!("`{runtime}` builds no images"))?;
    let dir = release_context()?;
    say(format!("$ {}", argv.join(" ")));
    match tokio::time::timeout(REBUILDING, run_step(&argv, &dir, &say)).await {
        Err(_) => Err(format!(
            "the build took longer than {}s",
            REBUILDING.as_secs()
        )),
        Ok(Err(status)) => Err(format!("the build failed: {status}")),
        Ok(Ok(_)) => Ok(()),
    }
}

/// The directory a release build runs in: [`RELEASE_CONTAINERFILE`] and
/// nothing else, so the context sent to the runtime is one small file. Under
/// the state directory rather than the system's temporary one, because
/// colima's `nerdctl` builds inside its VM and sees only what is shared with
/// it, which is the home directory.
fn release_context() -> Result<PathBuf, String> {
    let dir = crate::provider::Config::path_for_writing()
        .and_then(|config| config.parent().map(|state| state.join("worker-image")))
        .unwrap_or_else(|| std::env::temp_dir().join("luu-worker-image"));
    std::fs::create_dir_all(&dir)
        .map_err(|error| format!("creating {}: {error}", dir.display()))?;
    let file = dir.join("Containerfile.release");
    std::fs::write(&file, RELEASE_CONTAINERFILE)
        .map_err(|error| format!("writing {}: {error}", file.display()))?;
    Ok(dir)
}

#[derive(Debug, Clone, Serialize)]
pub struct TaskView {
    /// `build`, `install` or `start`.
    pub kind: &'static str,
    pub runtime: String,
    /// The image, for a build.
    pub image: Option<String>,
    /// `running`, `done` or `failed`.
    pub state: &'static str,
    /// The last lines it printed, both streams interleaved as they arrived.
    pub tail: Vec<String>,
    pub exit: Option<String>,
}

/// What this server ran for Settings → Runtimes, newest last: one per kind,
/// runtime and image, the latest replacing the one before it.
#[derive(Default)]
pub struct Tasks {
    all: Mutex<Vec<Arc<Mutex<TaskView>>>>,
}

fn held<T>(lock: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl Tasks {
    pub fn list(&self) -> Vec<TaskView> {
        held(&self.all)
            .iter()
            .map(|task| held(task).clone())
            .collect()
    }

    /// Starts `runtime build` of `image`, from the release or from source.
    ///
    /// From the release it is seconds: [`RELEASE_CONTAINERFILE`], written into
    /// a directory of its own under the state directory, fetches this
    /// version's Linux binary and checks it against the release's
    /// `SHA256SUMS`. Nothing in the base is read. From source it is
    /// `<base>/Containerfile`, which compiles luu inside the image and takes
    /// minutes, and exists only where the base is luu's own checkout.
    pub fn build(
        &self,
        runtime: Runtime,
        image: &str,
        from: BuildFrom,
        base: &Path,
    ) -> Result<(), String> {
        if from == BuildFrom::Source && !is_luu_checkout(base) {
            return Err(format!(
                "{} is not luu's own checkout, so there is no source to build the worker \
                 from: build it from the release",
                base.display()
            ));
        }
        let argv = build_line(runtime, image, from)
            .ok_or_else(|| format!("`{runtime}` builds no images"))?;
        if agent_core::worker::which(&argv[0]).is_none() {
            return Err(format!("`{}` is not installed on this machine", argv[0]));
        }
        let dir = match from {
            BuildFrom::Source => base.to_path_buf(),
            BuildFrom::Release => release_context()?,
        };
        self.start("build", runtime, Some(image), vec![argv], &dir)
    }

    /// Installs it, where [`install_steps`] has a line for it.
    pub fn install(&self, runtime: Runtime, base: &Path) -> Result<(), String> {
        let steps = install_steps(runtime).map_err(str::to_string)?;
        self.start("install", runtime, None, steps, base)
    }

    /// Starts its daemon, VM or service, where [`start_steps`] has a line.
    pub fn wake(&self, runtime: Runtime, base: &Path) -> Result<(), String> {
        let steps = start_steps(runtime)
            .ok_or_else(|| format!("`{runtime}` is not started from this page: it takes root"))?;
        self.start("start", runtime, None, steps, base)
    }

    /// Runs `steps` one after another, stopping at the first that fails,
    /// unless the same task is already running.
    fn start(
        &self,
        kind: &'static str,
        runtime: Runtime,
        image: Option<&str>,
        steps: Vec<Vec<String>>,
        base: &Path,
    ) -> Result<(), String> {
        let image = image.map(str::to_string);
        let same = |task: &TaskView| {
            task.kind == kind && task.runtime == runtime.as_str() && task.image == image
        };
        let mut all = held(&self.all);
        if all.iter().any(|task| {
            let task = held(task);
            same(&task) && task.state == "running"
        }) {
            return Err(format!("that {kind} is already running on {runtime}"));
        }
        let task = Arc::new(Mutex::new(TaskView {
            kind,
            runtime: runtime.as_str().to_string(),
            image: image.clone(),
            state: "running",
            tail: Vec::new(),
            exit: None,
        }));
        all.retain(|old| !same(&held(old)));
        all.push(task.clone());
        drop(all);
        tokio::spawn(run_steps(steps, base.to_path_buf(), task));
        Ok(())
    }
}

async fn run_steps(steps: Vec<Vec<String>>, base: PathBuf, task: Arc<Mutex<TaskView>>) {
    let note = |line: String| {
        let mut task = held(&task);
        task.tail.push(line);
        let over = task.tail.len().saturating_sub(TAIL);
        task.tail.drain(..over);
    };
    let finish = |state: &'static str, exit: String| {
        let mut task = held(&task);
        task.state = state;
        task.exit = Some(exit);
    };
    let mut last = String::new();
    for argv in &steps {
        note(format!("$ {}", argv.join(" ")));
        match run_step(argv, &base, &note).await {
            Ok(status) => last = status,
            Err(status) => return finish("failed", status),
        }
    }
    finish("done", last);
    tracing::info!(target: "luu::runtimes", steps = steps.len(), "a runtime task ended");
}

async fn run_step(argv: &[String], base: &Path, note: &impl Fn(String)) -> Result<String, String> {
    let mut command = tokio::process::Command::new(&argv[0]);
    command
        .args(&argv[1..])
        .current_dir(base)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    let mut child = command.spawn().map_err(|error| error.to_string())?;
    let (out, err) = (child.stdout.take(), child.stderr.take());
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    for stream in [
        out.map(|s| Box::new(s) as Box<dyn tokio::io::AsyncRead + Unpin + Send>),
        err.map(|s| Box::new(s) as Box<dyn tokio::io::AsyncRead + Unpin + Send>),
    ]
    .into_iter()
    .flatten()
    {
        let tx = tx.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stream).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if tx.send(line).is_err() {
                    return;
                }
            }
        });
    }
    drop(tx);
    while let Some(line) = rx.recv().await {
        note(line);
    }
    match child.wait().await {
        Ok(status) if status.success() => Ok(status.to_string()),
        Ok(status) => Err(status.to_string()),
        Err(error) => Err(error.to_string()),
    }
}

// ---- a session's runtime -------------------------------------------------------

/// How long the lines that start a runtime may take, together. Podman's
/// includes `machine init` where there is no machine yet, which downloads the
/// VM's image once.
const STARTING: Duration = Duration::from_secs(600);

/// Whether `runtime` is down in a way [`wake`] can fix: installed, silent to
/// its probe, and with a line to start it. Asked only after a worker failed to
/// start, so a healthy session pays nothing for it.
pub async fn asleep(runtime: Runtime) -> bool {
    let Some(argv) = runtime.probe_argv() else {
        return false;
    };
    if start_steps(runtime).is_none() || agent_core::worker::which(&argv[0]).is_none() {
        return false;
    }
    run(&argv).await.is_err()
}

/// Starts a runtime a session needs, and waits until it answers.
///
/// The lines are [`start_steps`], the ones *start it* runs in Settings, so
/// there is one definition of starting a runtime. Then the probe, once a
/// second for [`WAKING`]. Every step and every line it prints goes to `say`.
/// Nothing stops it afterwards: a podman machine or a colima VM is shared with
/// whatever else runs in it, and it is not luu's to stop. See
/// `RECORD/2026-10-27.runtimes-without-the-wait.completed.md`.
pub async fn wake(runtime: Runtime, say: impl Fn(String)) -> Result<(), String> {
    let steps = start_steps(runtime)
        .ok_or_else(|| format!("`{runtime}` is not started by luu: it takes root"))?;
    let probe = runtime
        .probe_argv()
        .ok_or_else(|| format!("`{runtime}` has nothing to start"))?;
    let here = std::env::temp_dir();
    let started = tokio::time::timeout(STARTING, async {
        for argv in &steps {
            say(format!("$ {}", argv.join(" ")));
            run_step(argv, &here, &say)
                .await
                .map_err(|status| format!("`{}` failed: {status}", argv.join(" ")))?;
        }
        Ok::<(), String>(())
    })
    .await;
    match started {
        Err(_) => {
            return Err(format!(
                "starting it took longer than {}s",
                STARTING.as_secs()
            ));
        }
        Ok(Err(error)) => return Err(error),
        Ok(Ok(())) => {}
    }
    let deadline = tokio::time::Instant::now() + WAKING;
    loop {
        match run(&probe).await {
            Ok(_) => return Ok(()),
            Err(said) if tokio::time::Instant::now() >= deadline => {
                return Err(format!(
                    "started, and still not answering after {}s: {said}",
                    WAKING.as_secs()
                ));
            }
            Err(_) => tokio::time::sleep(Duration::from_secs(1)).await,
        }
    }
}

// ---- policy files -------------------------------------------------------------

/// A policy file a posture may name, and where it would run.
#[derive(Debug, Clone, Serialize)]
pub struct PolicyFile {
    /// Relative to the base, the way a posture names it when `serve` runs
    /// there.
    pub path: String,
}

/// The `*.toml` files at the top of `base` that carry a `[sandbox]` or a
/// `[worker]` table. Not every TOML file: `Cargo.toml` parses as an empty
/// policy, and a list that offered it would offer a sandbox nobody wrote.
pub fn policy_files(base: &Path) -> Vec<PolicyFile> {
    let Ok(entries) = std::fs::read_dir(base) else {
        return Vec::new();
    };
    let mut found: Vec<PolicyFile> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_file() && path.extension().is_some_and(|ext| ext == "toml"))
        .filter(|path| {
            std::fs::read_to_string(path)
                .ok()
                .and_then(|text| text.parse::<toml::Table>().ok())
                .is_some_and(|table| table.contains_key("sandbox") || table.contains_key("worker"))
        })
        .filter_map(|path| {
            path.file_name().map(|name| PolicyFile {
                path: name.to_string_lossy().into_owned(),
            })
        })
        .collect();
    found.sort_by(|a, b| a.path.cmp(&b.path));
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_policy_file_is_one_that_says_so_and_cargo_toml_is_not() {
        let dir = std::env::temp_dir().join(format!("luu-policies-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a directory");
        std::fs::write(dir.join("Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();
        std::fs::write(dir.join("luu.toml"), "[sandbox]\nnetwork = false\n").unwrap();
        std::fs::write(dir.join("box.toml"), "[worker]\nruntime = \"docker\"\n").unwrap();
        std::fs::write(dir.join("broken.toml"), "[sandbox\n").unwrap();
        let found: Vec<String> = policy_files(&dir)
            .into_iter()
            .map(|file| file.path)
            .collect();
        assert_eq!(found, ["box.toml", "luu.toml"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_runtime_that_is_not_installed_is_not_asked() {
        // Apple's is on no Linux runner and not on this Mac.
        if agent_core::worker::which("container").is_some() {
            return;
        }
        let view = probe(Runtime::Container, &["luu-worker:dev".into()]).await;
        assert_eq!(view.path, None);
        assert_eq!(view.answers, None, "nothing was run: {view:?}");
        assert!(view.hint.is_some());
        assert_eq!(view.images[0].present, None);
    }

    #[test]
    fn only_the_two_brew_formulas_are_installed_from_the_page() {
        for refused in [Runtime::Docker, Runtime::Container, Runtime::Nerdctl] {
            assert!(install_steps(refused).is_err(), "{refused}");
        }
        for brewed in [Runtime::Podman, Runtime::Colima] {
            match install_steps(brewed) {
                Ok(steps) => {
                    assert_eq!(steps.len(), 1);
                    assert!(
                        steps[0][0].ends_with("brew") && steps[0][1] == "install",
                        "{steps:?}"
                    );
                }
                // Linux, or no Homebrew: said, not guessed.
                Err(why) => assert!(!why.is_empty()),
            }
        }
        // nerdctl's containerd is a system service; nothing starts it here.
        assert!(start_steps(Runtime::Nerdctl).is_none());
        assert_eq!(
            start_steps(Runtime::Colima).unwrap()[0],
            ["colima", "start", "--runtime", "containerd"]
        );
    }

    #[tokio::test]
    async fn a_task_runs_its_steps_in_order_and_stops_at_the_first_that_fails() {
        let tasks = Tasks::default();
        let dir = std::env::temp_dir();
        let sh = |script: &str| vec!["sh".to_string(), "-c".to_string(), script.to_string()];
        let steps = vec![sh("echo one"), sh("echo two; exit 3"), sh("echo three")];
        tasks
            .start("start", Runtime::Podman, None, steps, &dir)
            .expect("started");
        let ended = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let task = tasks.list().pop().expect("a task");
                if task.state != "running" {
                    return task;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("it ended");
        assert_eq!(ended.state, "failed");
        assert!(ended.tail.contains(&"one".to_string()) && ended.tail.contains(&"two".to_string()));
        assert!(
            !ended.tail.contains(&"three".to_string()),
            "{:?}",
            ended.tail
        );
    }

    #[test]
    fn a_source_build_needs_luu_s_own_checkout() {
        let tasks = Tasks::default();
        let dir = std::env::temp_dir().join(format!("luu-build-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a directory");
        let refused = tasks
            .build(Runtime::Docker, "luu-worker:dev", BuildFrom::Source, &dir)
            .unwrap_err();
        assert!(refused.contains("not luu's own checkout"), "{refused}");
        // A project with a Containerfile of its own is still not luu's: its
        // image must not be built under the worker's tag.
        std::fs::write(dir.join("Containerfile"), "FROM scratch\n").unwrap();
        assert!(!is_luu_checkout(&dir));
        let refused = tasks
            .build(Runtime::Docker, "luu-worker:dev", BuildFrom::Source, &dir)
            .unwrap_err();
        assert!(refused.contains("not luu's own checkout"), "{refused}");
        // And this repository is.
        let checkout = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        assert!(is_luu_checkout(&checkout));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The release's image is the source's image with another binary in it:
    /// the final stages agree, instruction for instruction, but for where
    /// `COPY --from` takes `luu` from. A package added to one and not the
    /// other is an image whose `commands` depend on how it was built.
    #[test]
    fn the_release_image_is_the_source_image_but_for_the_binary() {
        fn last_stage(file: &str) -> Vec<String> {
            // Instructions, with their continuation lines joined, from the
            // last `FROM` on.
            let mut instructions: Vec<String> = Vec::new();
            let mut open = false;
            for line in file.lines().map(str::trim) {
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                match open {
                    true => instructions.last_mut().unwrap().push_str(line),
                    false => instructions.push(line.to_string()),
                }
                open = line.ends_with('\\');
            }
            let from = instructions
                .iter()
                .rposition(|line| line.starts_with("FROM "))
                .expect("a FROM");
            instructions[from..]
                .iter()
                .map(|line| match line.starts_with("COPY --from=") {
                    // Where it lands, and not where it came from.
                    true => format!("COPY --from=… {}", line.rsplit(' ').next().unwrap()),
                    false => line.clone(),
                })
                .collect()
        }
        let source = last_stage(include_str!("../../../Containerfile"));
        assert_eq!(last_stage(RELEASE_CONTAINERFILE), source);
        assert!(
            source.contains(&"COPY --from=… /usr/local/bin/luu".to_string()),
            "{source:?}"
        );
        assert!(RELEASE_CONTAINERFILE.contains("sha256sum -c"));
    }

    #[test]
    fn a_label_is_found_wherever_the_runtime_put_it() {
        let docker = r#"[{"Id":"sha256:1","Config":{"Labels":{"luu.from":"release"}}}]"#;
        let podman = r#"[{"Labels":{"luu.from":"source"},"Config":{"Labels":null}}]"#;
        assert_eq!(label(docker, FROM_LABEL).as_deref(), Some("release"));
        assert_eq!(label(podman, FROM_LABEL).as_deref(), Some("source"));
        assert_eq!(label(r#"[{"Config":{"Labels":null}}]"#, FROM_LABEL), None);
        assert_eq!(label("not json", FROM_LABEL), None);
    }

    #[test]
    fn a_release_build_carries_this_version_and_reads_no_checkout() {
        let line = build_line(Runtime::Docker, "luu-worker:dev", BuildFrom::Release).unwrap();
        let version = format!("LUU_VERSION={}", env!("CARGO_PKG_VERSION"));
        assert!(line.contains(&version), "{line:?}");
        assert!(
            line.contains(&"Containerfile.release".to_string()),
            "{line:?}"
        );
        assert!(RELEASE_CONTAINERFILE.contains("ARG LUU_VERSION"));
        // And each says what it was built from, which is what decides whether
        // luu may rebuild it by itself.
        assert!(line.contains(&"luu.from=release".to_string()), "{line:?}");
        let line = build_line(Runtime::Docker, "luu-worker:dev", BuildFrom::Source).unwrap();
        assert!(!line.iter().any(|word| word == "--build-arg"), "{line:?}");
        assert!(line.contains(&"luu.from=source".to_string()), "{line:?}");
    }
}
