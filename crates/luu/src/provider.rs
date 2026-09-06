//! Where a model lives, written down once.
//!
//! A provider is a name for a destination — which backend, at which URL, with
//! which model and which key — kept in `config.toml` **in the state directory**
//! rather than in `luu.toml`. The split is the store's: `luu.toml` describes
//! *this project* and is committed with it, while a LAN address, a path to a key
//! and a model somebody pulled onto one disk are facts about *one machine*.
//!
//! ```toml
//! default = "local"
//!
//! [provider.local]
//! backend = "ollama"
//! url = "http://127.0.0.1:11434"
//! model = "qwen2.5-coder:7b"
//!
//! [provider.workstation]
//! backend = "openai"
//! url = "http://192.168.1.40:8080/v1"
//! model = "qwen2.5-coder-14b"
//! context-limit = 16384
//! ```
//!
//! **The one rule that is not bookkeeping**: the profile `default` names must
//! declare `remote = true` when its URL is not this machine, and must not
//! declare it when it is. `local-first` promises that *nothing leaves the
//! machine without the destination having been typed*, and a profile is a
//! destination typed once, weeks ago — except that `-p workstation` **is**
//! typing it, at the moment of use. The default is the only shape where nothing
//! was typed, so it is the only one that has to have been written down. A
//! `config.toml` that arrived from somewhere else fails to load if its default
//! points off the machine.
//!
//! See `RECORD/2026-09-07.naming-a-provider.completed.md`, and its later section for
//! why the rule is not on every profile.

use std::collections::BTreeMap;
use std::net::IpAddr;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The backends a flag or a profile can name.
///
/// One enum for both, so `--backend openai` and `backend = "openai"` cannot
/// drift into meaning different sets.
#[derive(Copy, Clone, Debug, PartialEq, Eq, clap::ValueEnum, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum BackendKind {
    Mock,
    Ollama,
    /// Any OpenAI-compatible server: `llama-server`, vLLM, LM Studio, a hosted
    /// endpoint, or Ollama's own `/v1`.
    Openai,
}

impl BackendKind {
    /// Where this backend is when nothing said otherwise. The mock is nowhere,
    /// which is why it is `None` rather than a URL nobody dials.
    fn default_url(self) -> Option<&'static str> {
        match self {
            Self::Mock => None,
            Self::Ollama => Some(DEFAULT_OLLAMA_URL),
            Self::Openai => Some(agent_core::backend::openai::DEFAULT_BASE_URL),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Mock => "mock",
            Self::Ollama => "ollama",
            Self::Openai => "openai",
        }
    }
}

pub const DEFAULT_OLLAMA_URL: &str = "http://127.0.0.1:11434";
pub const DEFAULT_MODEL: &str = "qwen2.5-coder:7b";
/// The file, inside whatever `crate::config` resolved.
pub const FILE: &str = "config.toml";

/// One named destination.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct Profile {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backend: Option<BackendKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// The window the *server* was started with, which on the OpenAI API is the
    /// only place it exists: there is no field on a request to send it in. A
    /// fact about the server rather than about the run, so it belongs beside the
    /// URL and not in a flag somebody retypes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_limit: Option<u32>,
    /// A path, never the key itself. A key in a config file is a key in every
    /// backup, every screen share and every `cat` in a bug report.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key_file: Option<PathBuf>,
    /// Declared, not derived — see the module note. Only load-bearing on the
    /// profile `default` names.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub remote: bool,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct File {
    #[serde(skip_serializing_if = "Option::is_none")]
    default: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    provider: BTreeMap<String, Profile>,
}

/// The file, parsed and checked.
#[derive(Debug, Clone, Default)]
pub struct Config {
    default: Option<String>,
    providers: BTreeMap<String, Profile>,
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("reading {path}: {source}")]
    Read {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("{path}: {source}")]
    Parse {
        path: String,
        #[source]
        source: toml::de::Error,
    },
    #[error("{path}: default = \"{name}\", and there is no [provider.{name}]")]
    NoSuchDefault { path: String, name: String },
    #[error(
        "{path}: [provider.{name}] is the default and {url} is not this machine, so every run \
         with no -p sends there. Add `remote = true` to say so, or name a local profile as \
         the default."
    )]
    UndeclaredRemoteDefault {
        path: String,
        name: String,
        url: String,
    },
    #[error(
        "{path}: [provider.{name}] says `remote = true` and {url} is this machine. The word is \
         there to be read, and one written where it is not true is one nobody reads."
    )]
    RemoteButLocal {
        path: String,
        name: String,
        url: String,
    },
    #[error("writing {path}: {source}")]
    Write {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("rendering the config: {message}")]
    Render { message: String },
    #[error("there is no [provider.{name}] in {path}")]
    NoSuchProfile { path: String, name: String },
    #[error("-p {name} was named, and this machine has no {file}")]
    NoConfigFile { name: String, file: String },
}

impl Config {
    /// The config file in the state directory, if there is one.
    ///
    /// Deliberately **not** [`crate::config::dir`]: that one asks the first-run
    /// question and creates the directory, and neither belongs to reading a
    /// file that is usually absent. A machine with no state directory has no
    /// providers, which is the same answer with none of the ceremony.
    pub fn load() -> Result<(Self, Option<PathBuf>), ConfigError> {
        let env = crate::config::Env::from_process();
        let crate::config::Resolution::Decided(dir) =
            crate::config::resolve(&env, |path| path.exists())
        else {
            return Ok((Self::default(), None));
        };
        let path = dir.join(FILE);
        if !path.exists() {
            return Ok((Self::default(), None));
        }
        let text = std::fs::read_to_string(&path).map_err(|source| ConfigError::Read {
            path: path.display().to_string(),
            source,
        })?;
        let config = Self::from_toml(&text, &path.display().to_string())?;
        Ok((config, Some(path)))
    }

    /// Parses and checks, with `path` only for what the errors say.
    pub fn from_toml(text: &str, path: &str) -> Result<Self, ConfigError> {
        let file: File = toml::from_str(text).map_err(|source| ConfigError::Parse {
            path: path.to_string(),
            source,
        })?;
        let config = Self {
            default: file.default,
            providers: file.provider,
        };
        config.check_default(path)?;
        Ok(config)
    }

    /// The whole of the load-time rule, and it is about one profile.
    fn check_default(&self, path: &str) -> Result<(), ConfigError> {
        let Some(name) = &self.default else {
            return Ok(());
        };
        let profile = self
            .providers
            .get(name)
            .ok_or_else(|| ConfigError::NoSuchDefault {
                path: path.to_string(),
                name: name.clone(),
            })?;
        let kind = profile.backend.unwrap_or(BackendKind::Ollama);
        // The mock reaches nothing, so it has no destination to declare.
        if kind == BackendKind::Mock {
            return Ok(());
        }
        let url = profile
            .url
            .clone()
            .or_else(|| kind.default_url().map(str::to_string))
            .unwrap_or_default();
        match (is_this_machine(&url), profile.remote) {
            (false, false) => Err(ConfigError::UndeclaredRemoteDefault {
                path: path.to_string(),
                name: name.clone(),
                url,
            }),
            (true, true) => Err(ConfigError::RemoteButLocal {
                path: path.to_string(),
                name: name.clone(),
                url,
            }),
            _ => Ok(()),
        }
    }

    /// The profile a run uses: the one `-p` named, or the file's default, or
    /// none at all.
    fn profile(
        &self,
        named: Option<&str>,
        path: &str,
    ) -> Result<Option<(&str, &Profile)>, ConfigError> {
        let name = match named {
            Some(name) => name,
            None => match &self.default {
                Some(name) => name.as_str(),
                None => return Ok(None),
            },
        };
        match self.providers.get_key_value(name) {
            Some((name, profile)) => Ok(Some((name.as_str(), profile))),
            None => Err(ConfigError::NoSuchProfile {
                path: path.to_string(),
                name: name.to_string(),
            }),
        }
    }
}

impl Config {
    pub fn default_name(&self) -> Option<&str> {
        self.default.as_deref()
    }

    pub fn profiles(&self) -> &BTreeMap<String, Profile> {
        &self.providers
    }

    /// The file this would be written to, without asking anything.
    ///
    /// **Never [`crate::config::dir`]**, for the reason that function gives
    /// about `luu stdio`: it prompts when nothing has been chosen, and a prompt
    /// inside a request handler is a hang with a socket attached. A machine that
    /// has not chosen a state directory has nowhere to put this, and says so.
    pub fn path_for_writing() -> Option<PathBuf> {
        let env = crate::config::Env::from_process();
        match crate::config::resolve(&env, |path| path.exists()) {
            crate::config::Resolution::Decided(dir) => Some(dir.join(FILE)),
            _ => None,
        }
    }

    /// TOML, as the file would be written.
    pub fn render(&self) -> Result<String, ConfigError> {
        toml::to_string_pretty(&File {
            default: self.default.clone(),
            provider: self.providers.clone(),
        })
        .map_err(|error| ConfigError::Render {
            message: error.to_string(),
        })
    }

    /// Replaces the file, and **only if what would replace it loads**.
    ///
    /// The check is [`Config::from_toml`] over the rendered text rather than a
    /// second copy of the rules: an editor that could write a file the loader
    /// refuses is an editor that bricks the next run, and the rule that a remote
    /// default must declare itself is the one it would brick it with.
    pub fn write(&self, path: &Path) -> Result<(), ConfigError> {
        let text = self.render()?;
        Self::from_toml(&text, &path.display().to_string())?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|source| ConfigError::Write {
                path: parent.display().to_string(),
                source,
            })?;
        }
        // Written beside and renamed over: a crash between the two leaves the
        // old file, and a truncated `config.toml` is a machine that cannot
        // reach a model until somebody edits TOML by hand.
        let temporary = path.with_extension("toml.new");
        std::fs::write(&temporary, &text).map_err(|source| ConfigError::Write {
            path: temporary.display().to_string(),
            source,
        })?;
        std::fs::rename(&temporary, path).map_err(|source| ConfigError::Write {
            path: path.display().to_string(),
            source,
        })
    }

    /// The file as an editor hands it back: a default and a set of profiles.
    pub fn from_parts(default: Option<String>, providers: BTreeMap<String, Profile>) -> Self {
        Self { default, providers }
    }
}

/// What the command line said, before the file is consulted. Every field is
/// optional because "the user did not say" is the question the profile answers,
/// and a clap default would have erased it.
#[derive(Debug, Default, Clone)]
pub struct Flags<'a> {
    pub provider: Option<&'a str>,
    pub backend: Option<BackendKind>,
    pub model: Option<&'a str>,
    pub ollama_url: Option<&'a str>,
    pub openai_url: Option<&'a str>,
    pub api_key_file: Option<&'a Path>,
    /// 0 is the CLI's "unknown window", which is also its "not set".
    pub context_limit: u32,
}

/// Where the window a run budgets against came from.
///
/// On the OpenAI API it cannot be *sent*, only budgeted against, so which of
/// the three answered is the difference between a number somebody chose for
/// this run and one a profile has been carrying since the server was started.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum WindowFrom {
    Flag,
    Profile,
    Unset,
}

/// One destination, resolved. Flags beat the profile field by field; the
/// profile beats the built-in defaults.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    pub kind: BackendKind,
    /// Empty for the mock, which is nowhere.
    pub url: String,
    pub model: String,
    pub key_file: Option<PathBuf>,
    pub context_limit: u32,
    pub window_from: WindowFrom,
    /// The profile this came from, when one did.
    pub profile: Option<String>,
}

impl Resolved {
    /// The destination a run has when nothing named one: the mock, which is
    /// nowhere. What a test that builds a server without going through the CLI
    /// wants, rather than a struct literal in three files that drift apart.
    pub fn mock() -> Self {
        Self {
            kind: BackendKind::Mock,
            url: String::new(),
            model: "mock".to_string(),
            key_file: None,
            context_limit: 0,
            window_from: WindowFrom::Unset,
            profile: None,
        }
    }

    /// The line a run prints before it sends anything.
    ///
    /// `None` for the mock: a line about a destination that does not exist is
    /// noise, and this one has to keep meaning something.
    pub fn line(&self) -> Option<String> {
        if self.kind == BackendKind::Mock {
            return None;
        }
        let destination = format!("{}@{}", self.kind.as_str(), authority_of(&self.url));
        let remote = match is_this_machine(&self.url) {
            true => "",
            false => " (remote)",
        };
        Some(match &self.profile {
            Some(name) => format!("provider: {name} → {destination}{remote}"),
            None => format!("provider: {destination}{remote}"),
        })
    }
}

/// The whole resolution: file, then profile, then flags.
pub fn resolve(
    config: &Config,
    path: Option<&Path>,
    flags: Flags<'_>,
) -> Result<Resolved, ConfigError> {
    let shown = path
        .map(|path| path.display().to_string())
        .unwrap_or_default();
    if let (Some(name), None) = (flags.provider, path) {
        return Err(ConfigError::NoConfigFile {
            name: name.to_string(),
            file: FILE.to_string(),
        });
    }
    let profile = config.profile(flags.provider, &shown)?;
    let (name, profile) = match profile {
        Some((name, profile)) => (Some(name.to_string()), profile.clone()),
        None => (None, Profile::default()),
    };

    let kind = flags
        .backend
        .or(profile.backend)
        .unwrap_or(BackendKind::Mock);
    // The two URL flags collapse here: only one of them can be the destination
    // once the backend is known, and carrying both further is how a run ends up
    // reporting the address it did not dial.
    let flag_url = match kind {
        BackendKind::Mock => None,
        BackendKind::Ollama => flags.ollama_url,
        BackendKind::Openai => flags.openai_url,
    };
    let url = flag_url
        .map(str::to_string)
        // A profile's URL is only its own backend's. `--backend openai` over a
        // profile that named an Ollama address would otherwise send an
        // OpenAI-shaped request to `/api/chat`.
        .or_else(|| match profile.backend.unwrap_or(kind) == kind {
            true => profile.url.clone(),
            false => None,
        })
        .or_else(|| kind.default_url().map(str::to_string))
        .unwrap_or_default();

    Ok(Resolved {
        kind,
        url,
        model: flags
            .model
            .map(str::to_string)
            .or(profile.model)
            .unwrap_or_else(|| DEFAULT_MODEL.to_string()),
        key_file: flags
            .api_key_file
            .map(Path::to_path_buf)
            .or(profile.key_file.map(|path| expand_home(&path))),
        context_limit: match flags.context_limit {
            0 => profile.context_limit.unwrap_or(0),
            named => named,
        },
        window_from: match (flags.context_limit, profile.context_limit) {
            (0, None) => WindowFrom::Unset,
            (0, Some(_)) => WindowFrom::Profile,
            _ => WindowFrom::Flag,
        },
        profile: name,
    })
}

/// `~` against `$HOME`, for a key path written in a file rather than typed.
fn expand_home(path: &Path) -> PathBuf {
    let Ok(rest) = path.strip_prefix("~") else {
        return path.to_path_buf();
    };
    match std::env::var_os("HOME") {
        Some(home) => PathBuf::from(home).join(rest),
        None => path.to_path_buf(),
    }
}

/// `host:port`, for the line a person reads — and for the host an editor
/// asks somebody to type back.
pub fn authority_of(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host)
}

/// Whether a URL names this machine.
///
/// **A box on the LAN does not.** The commitment says *the machine*, not *the
/// building*: `192.168.1.40` is somewhere the tokens travel to, and nothing
/// about the address says so until somebody writes it down.
///
/// A host this cannot parse is treated as remote. The failure that matters is
/// a destination reached without a declaration, so an address nobody can
/// classify asks for the declaration rather than skipping it.
pub fn is_this_machine(url: &str) -> bool {
    let authority = authority_of(url);
    let host = match authority.strip_prefix('[') {
        Some(rest) => rest.split_once(']').map_or(rest, |(host, _)| host),
        None => authority
            .split_once(':')
            .map_or(authority, |(host, _)| host),
    };
    match host.parse::<IpAddr>() {
        Ok(ip) => ip.is_loopback(),
        Err(_) => host.eq_ignore_ascii_case("localhost"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PATH: &str = "config.toml";

    fn config(text: &str) -> Result<Config, ConfigError> {
        Config::from_toml(text, PATH)
    }

    fn resolved(text: &str, flags: Flags<'_>) -> Resolved {
        let config = config(text).expect("the file loads");
        resolve(&config, Some(Path::new(PATH)), flags).expect("it resolves")
    }

    #[test]
    fn this_machine_is_loopback_and_a_lan_box_is_not() {
        assert!(is_this_machine("http://127.0.0.1:11434"));
        assert!(is_this_machine("http://localhost:8080/v1"));
        assert!(is_this_machine("http://[::1]:8080/v1"));
        assert!(is_this_machine("http://127.2.3.4:8080"));
        // The commitment says *the machine*, not *the building*.
        assert!(!is_this_machine("http://192.168.1.40:8080/v1"));
        assert!(!is_this_machine("https://openrouter.ai/api/v1"));
        // Unparseable is remote: the failure that matters is a destination
        // reached with no declaration, so it asks for one rather than skipping.
        assert!(!is_this_machine("nonsense"));
    }

    #[test]
    fn a_remote_default_must_say_so() {
        let error = config(
            r#"
            default = "hosted"
            [provider.hosted]
            backend = "openai"
            url = "https://openrouter.ai/api/v1"
            "#,
        )
        .expect_err("every run with no -p would send there");
        assert!(matches!(error, ConfigError::UndeclaredRemoteDefault { .. }));
        assert!(error.to_string().contains("remote = true"));
    }

    #[test]
    fn a_local_default_must_not_claim_to_be_remote() {
        let error = config(
            r#"
            default = "local"
            [provider.local]
            backend = "ollama"
            url = "http://127.0.0.1:11434"
            remote = true
            "#,
        )
        .expect_err("a word written where it is not true is one nobody reads");
        assert!(matches!(error, ConfigError::RemoteButLocal { .. }));
    }

    /// The narrowing the record's later section argued for: `-p hosted` is the
    /// destination being typed, so the declaration is only load-bearing on the
    /// profile nobody has to name.
    #[test]
    fn a_remote_profile_that_is_not_the_default_needs_no_declaration() {
        let text = r#"
            default = "local"
            [provider.local]
            backend = "ollama"
            [provider.hosted]
            backend = "openai"
            url = "https://openrouter.ai/api/v1"
            "#;
        let picked = resolved(
            text,
            Flags {
                provider: Some("hosted"),
                ..Flags::default()
            },
        );
        assert_eq!(picked.kind, BackendKind::Openai);
        assert_eq!(picked.url, "https://openrouter.ai/api/v1");
    }

    #[test]
    fn a_default_naming_nothing_is_a_load_error() {
        let error = config("default = \"gone\"\n").expect_err("there is no such profile");
        assert!(matches!(error, ConfigError::NoSuchDefault { .. }));
    }

    #[test]
    fn a_named_profile_that_is_not_there_is_refused_at_use() {
        let config = config("[provider.local]\nbackend = \"ollama\"\n").expect("it loads");
        let error = resolve(
            &config,
            Some(Path::new(PATH)),
            Flags {
                provider: Some("typo"),
                ..Flags::default()
            },
        )
        .expect_err("-p named nothing");
        assert!(matches!(error, ConfigError::NoSuchProfile { .. }));
    }

    #[test]
    fn with_no_file_and_no_flags_it_is_the_mock() {
        let picked = resolve(&Config::default(), None, Flags::default()).expect("it resolves");
        assert_eq!(picked.kind, BackendKind::Mock);
        assert_eq!(picked.model, DEFAULT_MODEL);
        assert_eq!(picked.line(), None, "the mock has nowhere to send");
    }

    #[test]
    fn naming_a_profile_with_no_file_says_which_file_is_missing() {
        let error = resolve(
            &Config::default(),
            None,
            Flags {
                provider: Some("hosted"),
                ..Flags::default()
            },
        )
        .expect_err("there is nowhere for that name to have come from");
        assert!(matches!(error, ConfigError::NoConfigFile { .. }));
    }

    #[test]
    fn the_default_profile_answers_when_no_flag_does() {
        let picked = resolved(
            r#"
            default = "local"
            [provider.local]
            backend = "ollama"
            url = "http://127.0.0.1:11500"
            model = "qwen2.5-coder:14b"
            context-limit = 16384
            "#,
            Flags::default(),
        );
        assert_eq!(picked.kind, BackendKind::Ollama);
        assert_eq!(picked.url, "http://127.0.0.1:11500");
        assert_eq!(picked.model, "qwen2.5-coder:14b");
        assert_eq!(picked.context_limit, 16384);
        assert_eq!(picked.profile.as_deref(), Some("local"));
    }

    #[test]
    fn a_flag_beats_the_profile_field_by_field() {
        let text = r#"
            default = "local"
            [provider.local]
            backend = "ollama"
            url = "http://127.0.0.1:11500"
            model = "qwen2.5-coder:14b"
            context-limit = 16384
            "#;
        let picked = resolved(
            text,
            Flags {
                model: Some("llama3:8b"),
                ..Flags::default()
            },
        );
        // The model moved and nothing else did: `-m` is one field of a
        // destination, not a destination.
        assert_eq!(picked.model, "llama3:8b");
        assert_eq!(picked.url, "http://127.0.0.1:11500");
        assert_eq!(picked.context_limit, 16384);

        let picked = resolved(
            text,
            Flags {
                context_limit: 4096,
                ..Flags::default()
            },
        );
        assert_eq!(picked.context_limit, 4096);

        let picked = resolved(
            text,
            Flags {
                ollama_url: Some("http://127.0.0.1:9999"),
                ..Flags::default()
            },
        );
        assert_eq!(picked.url, "http://127.0.0.1:9999");
    }

    /// A profile's URL belongs to the backend that profile named. Carrying it
    /// across would send an OpenAI-shaped request to Ollama's `/api/chat`.
    #[test]
    fn switching_backend_by_flag_drops_the_profiles_url() {
        let picked = resolved(
            r#"
            default = "local"
            [provider.local]
            backend = "ollama"
            url = "http://127.0.0.1:11500"
            "#,
            Flags {
                backend: Some(BackendKind::Openai),
                ..Flags::default()
            },
        );
        assert_eq!(picked.kind, BackendKind::Openai);
        assert_eq!(picked.url, agent_core::backend::openai::DEFAULT_BASE_URL);
    }

    #[test]
    fn the_line_names_the_profile_and_says_when_it_leaves_the_machine() {
        let text = r#"
            default = "local"
            [provider.local]
            backend = "ollama"
            [provider.hosted]
            backend = "openai"
            url = "https://openrouter.ai/api/v1"
            "#;
        assert_eq!(
            resolved(text, Flags::default()).line().unwrap(),
            "provider: local → ollama@127.0.0.1:11434"
        );
        assert_eq!(
            resolved(
                text,
                Flags {
                    provider: Some("hosted"),
                    ..Flags::default()
                }
            )
            .line()
            .unwrap(),
            "provider: hosted → openai@openrouter.ai (remote)"
        );
        // Typed rather than named: no profile, and the line still says where.
        let picked = resolve(
            &Config::default(),
            None,
            Flags {
                backend: Some(BackendKind::Openai),
                openai_url: Some("https://api.example.com/v1"),
                ..Flags::default()
            },
        )
        .expect("it resolves");
        assert_eq!(
            picked.line().unwrap(),
            "provider: openai@api.example.com (remote)"
        );
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("luu-config-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("the scratch directory");
        dir.join(FILE)
    }

    #[test]
    fn a_write_round_trips_through_the_loader() {
        let path = scratch("round-trip");
        let mut providers = BTreeMap::new();
        providers.insert(
            "local".to_string(),
            Profile {
                backend: Some(BackendKind::Ollama),
                url: Some("http://127.0.0.1:11434".to_string()),
                model: Some("qwen2.5-coder:7b".to_string()),
                ..Profile::default()
            },
        );
        let config = Config::from_parts(Some("local".to_string()), providers);
        config.write(&path).expect("it writes");

        let text = std::fs::read_to_string(&path).expect("it is there");
        let read = Config::from_toml(&text, PATH).expect("what was written loads");
        assert_eq!(read.default_name(), Some("local"));
        assert_eq!(
            read.profiles()["local"].url.as_deref(),
            Some("http://127.0.0.1:11434")
        );
        std::fs::remove_file(&path).ok();
    }

    /// The editor is checked by the loader that will read it back, rather than
    /// by a second copy of the rules — so a browser cannot save a file the next
    /// run refuses to start on.
    #[test]
    fn a_write_that_would_not_load_replaces_nothing() {
        let path = scratch("refused");
        std::fs::write(
            &path,
            "default = \"local\"\n\n[provider.local]\nbackend = \"ollama\"\n",
        )
        .expect("the file that is already there");

        let mut providers = BTreeMap::new();
        providers.insert(
            "hosted".to_string(),
            Profile {
                backend: Some(BackendKind::Openai),
                url: Some("https://openrouter.ai/api/v1".to_string()),
                ..Profile::default()
            },
        );
        let error = Config::from_parts(Some("hosted".to_string()), providers)
            .write(&path)
            .expect_err("an undeclared remote default is not a file this may write");
        assert!(matches!(error, ConfigError::UndeclaredRemoteDefault { .. }));

        let after = std::fs::read_to_string(&path).expect("still there");
        assert!(after.contains("[provider.local]"), "the old file survived");
        assert!(
            !path.with_extension("toml.new").exists(),
            "nothing is left half-written beside it"
        );
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn an_unknown_field_is_refused_rather_than_ignored() {
        let error = config("[provider.local]\nbackedn = \"ollama\"\n")
            .expect_err("a typo that is silently ignored is a profile that does nothing");
        assert!(matches!(error, ConfigError::Parse { .. }));
    }
}
