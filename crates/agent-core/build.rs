// Which sources the worker is built from, as `LUU_WORKER_BUILD`: a hash of
// every file under `src/`, this crate's `Cargo.toml`, and the workspace's
// `Cargo.toml` and `Cargo.lock` — path and bytes, in path order, FNV-1a.
//
// A content hash rather than a commit, because a commit cannot tell two
// uncommitted states apart, and because the image built from source has no
// `.git` to read (`.dockerignore`): it computes this over the same files its
// `COPY` brought in, so an image and the host that ordered it agree exactly
// when their sources do. agent-core's and not the whole binary's: `luu
// worker` is `serve_stdio(Tools::standard())`, all of it this crate, and a
// change to the server must not make every image stale. See
// RECORD/2026-10-27.an-image-that-knows-its-luu.completed.md.

use std::path::{Path, PathBuf};

fn main() {
    let crate_dir = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("cargo sets it"));
    let workspace = crate_dir.join("../..");

    let mut files: Vec<(String, PathBuf)> = Vec::new();
    walk(&crate_dir.join("src"), "crates/agent-core/src", &mut files);
    for (name, path) in [
        ("crates/agent-core/Cargo.toml", crate_dir.join("Cargo.toml")),
        ("Cargo.toml", workspace.join("Cargo.toml")),
        ("Cargo.lock", workspace.join("Cargo.lock")),
    ] {
        if path.is_file() {
            files.push((name.to_string(), path));
        }
    }
    files.sort();

    let mut hash = Fnv::default();
    for (name, path) in &files {
        let bytes = std::fs::read(path).unwrap_or_default();
        hash.write(name.as_bytes());
        hash.write(&[0]);
        hash.write(&(bytes.len() as u64).to_le_bytes());
        hash.write(&bytes);
    }
    println!("cargo:rustc-env=LUU_WORKER_BUILD={:016x}", hash.0);

    // Directories are scanned whole, so an edit anywhere under `src/` reruns
    // this, and agent-core recompiles then anyway.
    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-changed=Cargo.toml");
    println!("cargo:rerun-if-changed=build.rs");
    for name in ["Cargo.toml", "Cargo.lock"] {
        println!("cargo:rerun-if-changed={}", workspace.join(name).display());
    }
}

/// Every file under `dir`, named by `prefix` and its path below `dir` with `/`
/// between the parts, so the names are the same on every system.
fn walk(dir: &Path, prefix: &str, files: &mut Vec<(String, PathBuf)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = format!("{prefix}/{}", entry.file_name().to_string_lossy());
        if path.is_dir() {
            walk(&path, &name, files);
        } else if path.is_file() {
            files.push((name, path));
        }
    }
}

/// FNV-1a, 64 bits: stable across compilers and systems, which the standard
/// library's hasher does not promise, and enough to tell two trees apart.
struct Fnv(u64);

impl Default for Fnv {
    fn default() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }
}

impl Fnv {
    fn write(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 ^= u64::from(*byte);
            self.0 = self.0.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
}
