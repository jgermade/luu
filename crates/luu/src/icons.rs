//! A VSCode icon theme, loaded from wherever it is installed on this machine.
//!
//! **Nothing is vendored.** The alternative was shipping one theme's art in the
//! binary — measured at 1.5 MB and 263 files for `vscode-great-icons` — which
//! would tie the page to one person's taste and make this repository a
//! redistributor. The pattern used instead is the one `luu.toml` already uses
//! for `~/.cargo`: the theme lives on your machine, and you name it.
//!
//! ```toml
//! [ui]
//! icon-theme = "~/.vscode/extensions/emmanuelbeziat.vscode-great-icons-3.0.0"
//! ```
//!
//! Either an extension directory (whose `package.json` declares
//! `contributes.iconThemes`) or a theme JSON directly.
//!
//! **The id is the security design.** No path from the client ever reaches the
//! filesystem: the theme is read once into an id → absolute path table, and
//! only ids in that table can be served. There is no traversal to get wrong
//! because there is no path to traverse. See
//! `RECORD/2026-09-15.a-three-pane-inspector.WIP.md`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// What the page needs to pick an icon, by id.
///
/// The maps are the theme's own, passed through: matching a filename against
/// them is the page's job, and doing it here would mean a round trip per row.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Manifest {
    /// Absent when no theme is configured, so the page knows to draw its own
    /// two glyphs rather than to wait for icons that are not coming.
    pub loaded: bool,
    /// The theme's name, for the preferences panel to show.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub file_extensions: HashMap<String, String>,
    pub file_names: HashMap<String, String>,
    pub folder_names: HashMap<String, String>,
    /// The fallbacks the theme declares, when it declares them.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub folder_expanded: Option<String>,
}

/// A loaded theme: what the page is told, and where each id's bytes are.
#[derive(Debug, Clone, Default)]
pub struct Theme {
    pub manifest: Manifest,
    /// Id → the file on disk. The only paths this surface will ever open.
    paths: HashMap<String, PathBuf>,
}

impl Theme {
    /// The file for an id, or `None` for an id this theme never declared.
    pub fn path(&self, id: &str) -> Option<&Path> {
        self.paths.get(id).map(PathBuf::as_path)
    }
}

/// The subset of a VSCode icon theme this reads.
///
/// Everything else in the format — `light`, `highContrast`, `folderExpanded`
/// per folder, `hidesExplorerArrows` — is either a variant this page does not
/// render or a hint it does not need, and `serde` ignores what is not named.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ThemeFile {
    #[serde(default)]
    icon_definitions: HashMap<String, Definition>,
    #[serde(default)]
    file_extensions: HashMap<String, String>,
    #[serde(default)]
    file_names: HashMap<String, String>,
    #[serde(default)]
    folder_names: HashMap<String, String>,
    file: Option<String>,
    folder: Option<String>,
    folder_expanded: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Definition {
    icon_path: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExtensionManifest {
    #[serde(default)]
    contributes: Contributes,
    display_name: Option<String>,
    name: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Contributes {
    #[serde(default)]
    icon_themes: Vec<IconThemeEntry>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IconThemeEntry {
    path: String,
    label: Option<String>,
}

/// Finds the theme JSON, given what the person wrote in `config.toml`.
///
/// A directory is an installed extension: its `package.json` says which of its
/// files is the theme, and a theme that declares several gets the first, which
/// is the one VSCode shows first too.
fn locate(named: &Path) -> Result<(PathBuf, Option<String>), String> {
    if named.is_file() {
        return Ok((named.to_path_buf(), None));
    }
    if !named.is_dir() {
        return Err(format!("{}: no such file or directory", named.display()));
    }
    let package = named.join("package.json");
    let raw = std::fs::read_to_string(&package)
        .map_err(|error| format!("{}: {error}", package.display()))?;
    let manifest: ExtensionManifest =
        serde_json::from_str(&raw).map_err(|error| format!("{}: {error}", package.display()))?;
    let entry = manifest
        .contributes
        .icon_themes
        .into_iter()
        .next()
        .ok_or_else(|| {
            format!(
                "{}: declares no icon theme (contributes.iconThemes is empty)",
                package.display()
            )
        })?;
    let label = entry.label.or(manifest.display_name).or(manifest.name);
    Ok((named.join(entry.path), label))
}

/// Reads a theme, resolving every icon path against the theme file's own
/// directory.
///
/// A definition whose file is missing is dropped rather than failing the load:
/// themes carry entries for icons they no longer ship, and one stale line
/// should not cost a person every icon.
pub fn load(named: &Path) -> Result<Theme, String> {
    let (theme_path, label) = locate(named)?;
    let base = theme_path
        .parent()
        .ok_or_else(|| format!("{}: has no directory", theme_path.display()))?
        .to_path_buf();
    let raw = std::fs::read_to_string(&theme_path)
        .map_err(|error| format!("{}: {error}", theme_path.display()))?;
    // VSCode's own themes are JSON with comments often enough that a strict
    // parser is the wrong tool; `serde_json` is strict, so a theme that uses
    // them fails with a message naming the line, which is a better answer than
    // silently half-loading.
    let theme: ThemeFile =
        serde_json::from_str(&raw).map_err(|error| format!("{}: {error}", theme_path.display()))?;

    let mut paths = HashMap::new();
    for (id, definition) in &theme.icon_definitions {
        let Some(icon_path) = &definition.icon_path else {
            continue;
        };
        let resolved = base.join(icon_path.trim_start_matches("./"));
        // Canonicalized, and that is what makes the table trustworthy: an
        // `iconPath` of `../../../etc/shadow` resolves to a real path here and
        // is then refused for being outside the theme's own directory.
        let Ok(resolved) = resolved.canonicalize() else {
            continue;
        };
        let Ok(base_real) = base.canonicalize() else {
            continue;
        };
        if !resolved.starts_with(&base_real) {
            continue;
        }
        paths.insert(id.clone(), resolved);
    }

    // A map entry pointing at an id with no file is dropped too, so the page
    // never asks for an icon that cannot answer.
    let keep = |map: HashMap<String, String>| -> HashMap<String, String> {
        map.into_iter()
            .filter(|(_, id)| paths.contains_key(id))
            .collect()
    };
    let keep_one = |id: Option<String>| id.filter(|id| paths.contains_key(id));

    Ok(Theme {
        manifest: Manifest {
            loaded: true,
            name: label,
            file_extensions: keep(theme.file_extensions),
            file_names: keep(theme.file_names),
            folder_names: keep(theme.folder_names),
            file: keep_one(theme.file),
            folder: keep_one(theme.folder),
            folder_expanded: keep_one(theme.folder_expanded),
        },
        paths,
    })
}

/// The content type for an icon, by extension. Small and closed: these are the
/// three things an icon theme ships, and guessing beyond them would mean
/// serving whatever a theme put in its directory as whatever it claimed.
pub fn content_type(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn theme_dir() -> (tempdir::Dir, PathBuf) {
        let dir = tempdir::Dir::new("icons");
        let icons = dir.path().join("icons");
        fs::create_dir_all(&icons).unwrap();
        fs::write(icons.join("rust.svg"), "<svg/>").unwrap();
        fs::write(icons.join("file.svg"), "<svg/>").unwrap();
        fs::write(
            dir.path().join("theme.json"),
            r#"{
              "iconDefinitions": {
                "_rust": { "iconPath": "./icons/rust.svg" },
                "_file": { "iconPath": "./icons/file.svg" },
                "_gone": { "iconPath": "./icons/missing.svg" },
                "_escape": { "iconPath": "../../../etc/hosts" }
              },
              "fileExtensions": { "rs": "_rust", "zzz": "_gone" },
              "fileNames": { "Cargo.toml": "_rust" },
              "file": "_file"
            }"#,
        )
        .unwrap();
        let path = dir.path().join("theme.json");
        (dir, path)
    }

    #[test]
    fn a_theme_json_loads_and_keeps_only_icons_that_exist() {
        let (_dir, path) = theme_dir();
        let theme = load(&path).expect("theme loads");
        assert!(theme.manifest.loaded);
        assert_eq!(theme.manifest.file_extensions.get("rs").unwrap(), "_rust");
        // The definition whose file is missing is dropped, and so is the map
        // entry that pointed at it — the page never asks for a dead id.
        assert!(!theme.manifest.file_extensions.contains_key("zzz"));
        assert!(theme.path("_gone").is_none());
    }

    /// The one that matters: an `iconPath` that climbs out of the theme's own
    /// directory is not servable, however real the file it names is.
    #[test]
    fn an_icon_path_outside_the_theme_is_refused() {
        let (_dir, path) = theme_dir();
        let theme = load(&path).expect("theme loads");
        assert!(
            theme.path("_escape").is_none(),
            "a theme must not be able to hand out /etc/hosts"
        );
    }

    #[test]
    fn an_extension_directory_is_read_through_its_package_json() {
        let (dir, _) = theme_dir();
        fs::write(
            dir.path().join("package.json"),
            r#"{
              "name": "great-icons",
              "displayName": "Great Icons",
              "contributes": { "iconThemes": [{ "id": "g", "label": "Great", "path": "./theme.json" }] }
            }"#,
        )
        .unwrap();
        let theme = load(dir.path()).expect("extension loads");
        assert_eq!(theme.manifest.name.as_deref(), Some("Great"));
        assert!(theme.path("_rust").is_some());
    }

    #[test]
    fn a_path_that_is_not_there_says_so() {
        let error = load(Path::new("/nonexistent/theme")).unwrap_err();
        assert!(error.contains("no such file"), "{error}");
    }

    /// A tiny scratch directory, removed on drop. Beside the tests that use it
    /// rather than pulled in as a dependency: this is the only place in the
    /// crate that wants one.
    mod tempdir {
        use std::path::{Path, PathBuf};

        pub struct Dir(PathBuf);

        impl Dir {
            pub fn new(tag: &str) -> Self {
                let unique = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos();
                let path = std::env::temp_dir().join(format!("luu-{tag}-{unique}"));
                std::fs::create_dir_all(&path).unwrap();
                Self(path)
            }

            pub fn path(&self) -> &Path {
                &self.0
            }
        }

        impl Drop for Dir {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
    }
}
