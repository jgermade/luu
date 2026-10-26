//! Brand marks: the logos the pickers draw beside a provider or a model.
//!
//! The page carries a few of its own (`web/brands/`, from Simple Icons, CC0).
//! This is the other half: **a pack** named in `config.toml`, drawn instead —
//! a folder of `<slug>.svg`, or a folder with an `icons/` folder of them,
//! which is how `@lobehub/icons-static-svg` is laid out.
//!
//! ```toml
//! [ui]
//! brand-icons = "~/.luu/brand-icons/lobehub"
//! ```
//!
//! **The slug is the security design**, as the id is for the tree's icons
//! (`crate::icons`): the pack is read once into a slug → file table, a slug
//! is `[a-z0-9][a-z0-9.-]*`, and only slugs in the table are served — as SVG,
//! and nothing else. See `RECORD/2026-10-26.brand-icons.completed.md`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Serialize;

/// A loaded pack: its name, and where each slug's file is.
#[derive(Debug, Clone, Default)]
pub struct Pack {
    pub name: String,
    /// Moves every time the server swaps the pack, and the page puts it in
    /// each mark's URL: they are served cacheable.
    pub revision: u64,
    icons: BTreeMap<String, PathBuf>,
}

impl Pack {
    /// The file for a slug, or `None` for one this pack does not have.
    pub fn path(&self, slug: &str) -> Option<&Path> {
        self.icons.get(slug).map(PathBuf::as_path)
    }

    /// Every slug it has, sorted: what the page asks before it draws one.
    pub fn slugs(&self) -> Vec<String> {
        self.icons.keys().cloned().collect()
    }
}

/// A slug as a pack's file names one: lowercase letters, digits, `.` and `-`,
/// led by a letter or a digit — `claude`, `claude-color`, `meta-brand`.
pub fn is_slug(text: &str) -> bool {
    text.chars()
        .next()
        .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        && text
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '-'))
}

/// Reads a pack: every `<slug>.svg` directly in `dir`, or in `dir/icons` when
/// that is a folder. A pack with none is refused — it would draw nothing
/// under a name that says it draws something.
pub fn load(dir: &Path) -> Result<Pack, String> {
    let inner = dir.join("icons");
    let from = match inner.is_dir() {
        true => inner,
        false => dir.to_path_buf(),
    };
    let entries =
        std::fs::read_dir(&from).map_err(|error| format!("{}: {error}", from.display()))?;
    let icons: BTreeMap<String, PathBuf> = entries
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|t| t.is_file()))
        .filter_map(|entry| {
            let path = entry.path();
            let slug = path
                .file_name()?
                .to_str()?
                .strip_suffix(".svg")?
                .to_string();
            is_slug(&slug).then_some((slug, path))
        })
        .collect();
    if icons.is_empty() {
        return Err(format!("{} has no <name>.svg in it", from.display()));
    }
    Ok(Pack {
        name: name_of(dir),
        revision: 0,
        icons,
    })
}

/// What a pack is called on the page: an npm package's own name where the
/// folder is one, the folder's name otherwise.
fn name_of(dir: &Path) -> String {
    #[derive(serde::Deserialize)]
    struct Package {
        name: Option<String>,
    }
    std::fs::read_to_string(dir.join("package.json"))
        .ok()
        .and_then(|text| serde_json::from_str::<Package>(&text).ok())
        .and_then(|package| package.name)
        .unwrap_or_else(|| {
            dir.file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| dir.display().to_string())
        })
}

/// A pack Settings can offer: where it is, what it is called, how many marks
/// it has, and where it was found — `luu` for one imported into luu's own
/// folder, `npm` for one installed beside the page's other optional packages.
#[derive(Debug, Clone, Serialize)]
pub struct Found {
    pub path: String,
    pub name: String,
    pub count: usize,
    pub source: &'static str,
}

/// The npm packages of marks luu knows the layout of, under `node_modules`.
const NPM: [&str; 1] = ["@lobehub/icons-static-svg"];

/// Every pack on this machine: each folder under `imported_into` that loads,
/// then each known npm package under `node_modules` that is installed.
pub fn discover(imported_into: Option<&Path>, node_modules: &Path) -> Vec<Found> {
    let mut found = Vec::new();
    if let Some(root) = imported_into
        && let Ok(entries) = std::fs::read_dir(root)
    {
        let mut dirs: Vec<PathBuf> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.is_dir())
            .filter(|path| {
                path.file_name()
                    .is_some_and(|name| !name.to_string_lossy().starts_with('.'))
            })
            .collect();
        dirs.sort();
        found.extend(dirs.iter().filter_map(|dir| offered(dir, "luu")));
    }
    found.extend(
        NPM.iter()
            .filter_map(|package| offered(&node_modules.join(package), "npm")),
    );
    found
}

fn offered(dir: &Path, source: &'static str) -> Option<Found> {
    let pack = load(dir).ok()?;
    // Canonical, so the path Settings writes reads as the place it is, and
    // compares equal to the one `config.toml` names however that was spelled.
    let dir = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    Some(Found {
        path: dir.display().to_string(),
        name: pack.name,
        count: pack.icons.len(),
        source,
    })
}

/// Writes a picked folder of marks under `root`, only if it loads as a pack.
/// The folder checks are the icon themes' own — see [`crate::icons::import_folder`].
pub fn import(root: &Path, files: Vec<(String, Vec<u8>)>) -> Result<PathBuf, String> {
    crate::icons::import_folder(root, files, |staging| load(staging).map(|_| ()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("luu-brands-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A slug is a name and never a path, so nothing a page asks for can
    /// leave the table.
    #[test]
    fn a_slug_is_a_name() {
        for slug in ["claude", "claude-color", "meta-brand", "01-ai", "qwen2.5"] {
            assert!(is_slug(slug), "{slug}");
        }
        for slug in ["", "../x", "a/b", ".hidden", "-x", "Claude", "a b"] {
            assert!(!is_slug(slug), "{slug:?}");
        }
    }

    /// lobehub's layout and a flat folder both load, by their files' names
    /// alone; an empty folder does not.
    #[test]
    fn a_pack_is_a_folder_of_svgs_or_one_with_icons_in_it() {
        let flat = scratch("flat");
        std::fs::write(flat.join("claude.svg"), "<svg/>").unwrap();
        std::fs::write(flat.join("Not A Slug.svg"), "<svg/>").unwrap();
        std::fs::write(flat.join("notes.txt"), "no").unwrap();
        let pack = load(&flat).unwrap();
        assert_eq!(pack.slugs(), vec!["claude".to_string()]);
        assert!(
            pack.path("claude")
                .is_some_and(|p| p.ends_with("claude.svg"))
        );
        assert!(pack.path("../claude").is_none());

        let npm = scratch("npm");
        std::fs::create_dir_all(npm.join("icons")).unwrap();
        std::fs::write(npm.join("icons/qwen.svg"), "<svg/>").unwrap();
        std::fs::write(
            npm.join("package.json"),
            r#"{"name": "@lobehub/icons-static-svg"}"#,
        )
        .unwrap();
        let pack = load(&npm).unwrap();
        assert_eq!(pack.name, "@lobehub/icons-static-svg");
        assert_eq!(pack.slugs(), vec!["qwen".to_string()]);

        assert!(load(&scratch("empty")).is_err());
        for dir in [flat, npm] {
            let _ = std::fs::remove_dir_all(dir);
        }
    }

    /// What luu imported and what npm installed are both offered, and a
    /// folder that does not load is not.
    #[test]
    fn packs_are_found_where_they_are_imported_and_installed() {
        let root = scratch("found");
        let imported = root.join("brand-icons");
        std::fs::create_dir_all(imported.join("mine")).unwrap();
        std::fs::write(imported.join("mine/ollama.svg"), "<svg/>").unwrap();
        std::fs::create_dir_all(imported.join("broken")).unwrap();
        let modules = root.join("node_modules");
        std::fs::create_dir_all(modules.join("@lobehub/icons-static-svg/icons")).unwrap();
        std::fs::write(
            modules.join("@lobehub/icons-static-svg/icons/meta.svg"),
            "<svg/>",
        )
        .unwrap();

        let found = discover(Some(&imported), &modules);
        let seen: Vec<(&str, &str, usize)> = found
            .iter()
            .map(|f| (f.name.as_str(), f.source, f.count))
            .collect();
        assert_eq!(
            seen,
            vec![("mine", "luu", 1), ("icons-static-svg", "npm", 1)]
        );
        let _ = std::fs::remove_dir_all(root);
    }
}
