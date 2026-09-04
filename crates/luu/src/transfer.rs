//! `luu transfer` and `luu import`: a session moving between hosts.
//!
//! The format and the border rule are in [`agent_core::transfer`]; this is the
//! half that touches disk — writing the two files of a bundle, reading them
//! back, and putting what arrives into this host's store.
//!
//! **What crosses is the stream.** `transfer` reads the session's own record
//! lines out of the store (or a `--record` file) and copies them; it never
//! renders a `SessionView` into a file. The fold is a cache of the stream, and
//! shipping a cache instead of the thing it caches is the snapshot
//! `RECORD/2026-08-31.the-portal-and-the-gate.completed.md` refused.
//!
//! **What does not cross is authority.** Every job that is not `Closed` returns
//! to this host's gate, and a plan this host's `luu.toml` does not grant arrives
//! refused. The destination appends one `imported` line saying so, folds the
//! whole stream, and stores the result — so the view beside the stream is still
//! a fold of it, which is the one invariant the store has.
//!
//! Argued in `RECORD/2026-09-04.the-border-and-the-gate.completed.md`.

use std::path::{Path, PathBuf};

use agent_core::api::SessionView;
use agent_core::protocol::ServerMessage;
use agent_core::record::RecordLine;
use agent_core::sandbox::Sandbox;
use agent_core::transfer::{
    self, ImportedJob, Manifest, Origin, ResolvedSandbox, manifest_path, record_path, regate,
};
use anyhow::{Context, Result, bail};

use crate::session::now_ms;
use crate::store::SessionStore;

/// Where the lines of a transfer came from, so the two commands can say it in
/// their own words.
pub enum Source {
    /// A session in this host's store.
    Store { path: PathBuf, id: String },
    /// A `--record` file, as `--record` or `luu chat --record` wrote one. The
    /// file stem names the session, the way `luu export` names one.
    Record(PathBuf),
}

/// Writes a bundle: the stream verbatim, and the envelope beside it.
pub fn write(source: &Source, sandbox: &Sandbox, out: &Path) -> Result<(String, usize)> {
    let (id, lines) = match source {
        Source::Store { path, id } => {
            // Checked before opening, because opening *creates*: a transfer that
            // named the wrong store would otherwise leave an empty one behind
            // and report a missing session rather than a missing store.
            if !path.exists() {
                bail!("no session store at {}", path.display());
            }
            let store = SessionStore::open(path)
                .with_context(|| format!("opening the session store at {}", path.display()))?;
            if store.load(id)?.is_none() {
                bail!("no session `{id}` in {}", path.display());
            }
            let lines = store.stream(id)?;
            // A session stored before the stream was kept. Refused rather than
            // reconstructed: what is there is a fold, and folding it back into
            // a stream would invent line orders and timings the session never
            // had. See the record.
            if lines.is_empty() {
                bail!(
                    "session `{id}` has no stream in {}: it was stored before luu kept one, so \
                     all that is on disk is the fold. A fold is not a stream and this will not \
                     ship one that never existed.",
                    path.display(),
                );
            }
            (id.clone(), lines)
        }
        Source::Record(path) => {
            let id = path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .context("a record file needs a name")?
                .to_string();
            (id, crate::export::read_record(path)?)
        }
    };

    let origin = Origin {
        host: transfer::hostname(),
        session: id.clone(),
        sandbox: ResolvedSandbox::from(sandbox),
    };
    let manifest = Manifest::new(origin, now_ms());

    std::fs::create_dir_all(out).with_context(|| format!("creating {}", out.display()))?;
    let manifest_file = manifest_path(out);
    std::fs::write(
        &manifest_file,
        format!("{}\n", serde_json::to_string_pretty(&manifest)?),
    )
    .with_context(|| format!("writing {}", manifest_file.display()))?;

    let mut stream = String::new();
    for line in &lines {
        stream.push_str(&serde_json::to_string(line)?);
        stream.push('\n');
    }
    let record_file = record_path(out);
    std::fs::write(&record_file, stream)
        .with_context(|| format!("writing {}", record_file.display()))?;

    Ok((id, lines.len()))
}

/// A bundle, read and checked but not yet imported.
pub struct Bundle {
    pub manifest: Manifest,
    pub lines: Vec<RecordLine>,
}

/// Reads one, refusing on the envelope before the stream is parsed.
pub fn read(bundle: &Path) -> Result<Bundle> {
    let manifest_file = manifest_path(bundle);
    let text = std::fs::read_to_string(&manifest_file)
        .with_context(|| format!("reading {}", manifest_file.display()))?;
    let manifest: Manifest = serde_json::from_str(&text)
        .with_context(|| format!("{} is not a transfer manifest", manifest_file.display()))?;
    manifest.check()?;

    let lines = crate::export::read_record(&record_path(bundle))?;
    // The envelope and the stream have to agree about what they are. They are
    // written together and a bundle where they disagree has been edited.
    if let Some(RecordLine::Header {
        format, protocol, ..
    }) = lines.first()
        && (*format != manifest.format || *protocol != manifest.protocol)
    {
        bail!(
            "the manifest says protocol {} format {} and the stream's header says {protocol}/{format}",
            manifest.protocol,
            manifest.format,
        );
    }
    Ok(Bundle { manifest, lines })
}

/// What an import did, for the person who ran it to read.
pub struct Imported {
    pub id: String,
    pub view: SessionView,
    pub jobs: Vec<ImportedJob>,
}

/// Folds a bundle, sends its open jobs back to the gate, and stores both halves.
///
/// The `imported` line is appended to the stream *before* the fold is taken, so
/// what the store holds beside the stream is a fold of exactly the stream it
/// holds — the border is in the account rather than applied on top of it.
pub fn import(
    bundle: &Bundle,
    sandbox: &Sandbox,
    store_path: &Path,
    id: Option<&str>,
) -> Result<Imported> {
    let id = id.unwrap_or(&bundle.manifest.origin.session).to_string();
    let mut store = SessionStore::open(store_path)
        .with_context(|| format!("opening the session store at {}", store_path.display()))?;
    if store.load(&id)?.is_some() {
        bail!(
            "this host already has a session called `{id}`. Importing over it would fork a \
             session under one name; pass --as <id> to give this one another.",
        );
    }

    // Folded once to ask what the border has to do, then again with the answer
    // in the stream — the second fold is the one that is stored.
    let arrived = SessionView::from_record(&id, &bundle.lines);
    let jobs = regate(&arrived, sandbox);
    let at_ms = bundle.lines.iter().fold(0, |latest, line| match line {
        RecordLine::Protocol { at_ms, .. } | RecordLine::Trace { at_ms, .. } => latest.max(*at_ms),
        RecordLine::Header { .. } => latest,
    });

    let mut lines = bundle.lines.clone();
    lines.push(RecordLine::Protocol {
        at_ms,
        message: ServerMessage::Imported {
            from: bundle.manifest.origin.clone(),
            jobs: jobs.clone(),
        },
    });

    let view = SessionView::from_record(&id, &lines);
    store.append(&id, &lines)?;
    store.save(&view)?;

    Ok(Imported { id, view, jobs })
}

/// What the person approving on this host is being asked to judge: what the job
/// could reach over there, and what it can reach here.
pub fn difference(origin: &ResolvedSandbox, here: &Sandbox) -> String {
    let here = ResolvedSandbox::from(here);
    format!(
        "the origin's sandbox\n{}\nthis host's sandbox\n{}",
        indent(&origin.describe()),
        indent(&here.describe()),
    )
}

fn indent(text: &str) -> String {
    text.lines()
        .map(|line| format!("  {line}\n"))
        .collect::<String>()
}
