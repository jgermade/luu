//! Email: the code a user logs in with. The only mail luu sends.
//!
//! **Settings are saved only once they have worked.** The first code — the
//! owner's — goes out under the settings as typed, and they are written to
//! `[auth.smtp]` and `smtp.password` only when that code comes back. So the
//! code is the test of the mail and of the mailbox at once. See
//! `RECORD/2026-11-03.an-owner-proved-by-email.WIP.md`.
//!
//! **The password is a file, luu's or the cluster's.** Typed, luu writes it
//! to `smtp.password`, `0600`. Mounted — a Kubernetes or Docker secret — it
//! stays where it is and `password-ref = "file:/path"` names it, read on every
//! send. Never an environment variable: every command a session runs inherits
//! luu's environment, and on a public server those are other people's.
//!
//! **A private CA is a file, added to the public roots.** `ca-file` names a
//! PEM whose certificates the mail server's may chain to, beside Mozilla's and
//! for this host alone. See
//! `RECORD/2026-11-11.a-ca-file-for-the-mail-server.completed.md`.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use lettre::message::header::ContentType;
use lettre::transport::smtp::authentication::Credentials;
use lettre::transport::smtp::client::{Certificate, Tls, TlsParameters};
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};

use crate::provider::SmtpTable;

/// Mail settings with the password read, ready to send.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Mail {
    pub url: String,
    pub from: String,
    pub user: Option<String>,
    pub password: Option<String>,
    /// The file the password was read from when it is not luu's to write:
    /// a mounted secret. Saved as `password-ref`, and nothing is copied.
    pub mounted: Option<PathBuf>,
    /// A PEM of certificates trusted beside the public roots, for this
    /// server: `ca-file`. Read on every send.
    pub ca_file: Option<PathBuf>,
}

/// Where the password goes, in the state directory.
pub const PASSWORD_FILE: &str = "smtp.password";

impl Mail {
    /// The settings `config.toml` has, the password read from its file.
    pub fn from_table(table: &SmtpTable) -> Result<Self> {
        let (password, mounted) = match (&table.password_file, &table.password_ref) {
            (Some(_), Some(_)) => anyhow::bail!(
                "[auth.smtp] names the password twice: password-file is the one luu wrote, \
                 password-ref one mounted for it. Keep one"
            ),
            (Some(path), None) => {
                let path = crate::provider::expand_home(path);
                refuse_where_commands_read(&path)?;
                let password = crate::secret::read(
                    &path,
                    "the SMTP password file",
                    "hands the mail account to anyone who can read it",
                )?;
                (Some(password), None)
            }
            (None, Some(reference)) => {
                let path = reference_path(reference)?;
                (Some(read_mounted(&path)?), Some(path))
            }
            (None, None) => (None, None),
        };
        Ok(Self {
            url: table.url.clone(),
            from: table.from.clone(),
            user: table.user.clone(),
            password,
            mounted,
            ca_file: table.ca_file.clone(),
        })
    }

    /// The same settings, trusting the certificates in `path` as well.
    pub fn with_ca_file(self, path: Option<PathBuf>) -> Self {
        Self {
            ca_file: path,
            ..self
        }
    }

    /// The certificates `ca-file` names, read now. A file that is not
    /// there, is relative, or holds no certificate is a refusal naming it —
    /// never the public roots alone, which would report a wrong path as a
    /// server nobody signed.
    fn extra_roots(&self) -> Result<Option<Certificate>> {
        let Some(path) = &self.ca_file else {
            return Ok(None);
        };
        let path = crate::provider::expand_home(path);
        if !path.is_absolute() {
            anyhow::bail!(
                "ca-file `{}` is not an absolute path: luu reads it wherever it was started",
                path.display()
            );
        }
        let pem = std::fs::read(&path)
            .with_context(|| format!("reading the mail server's CA from {}", path.display()))?;
        if !pem
            .windows(27)
            .any(|window| window == b"-----BEGIN CERTIFICATE-----")
        {
            anyhow::bail!(
                "{} holds no certificate: a CA file is PEM, one or more \
                 `-----BEGIN CERTIFICATE-----` blocks",
                path.display()
            );
        }
        let certificate = Certificate::from_pem(&pem)
            .with_context(|| format!("{} is not a PEM of certificates", path.display()))?;
        Ok(Some(certificate))
    }

    /// The transport's TLS with `roots` trusted beside the public ones: what
    /// `from_url` chose for this URL, made again with them. The name checked
    /// is still the URL's host.
    fn tls_with(&self, roots: Certificate) -> Result<Tls> {
        let parsed = reqwest::Url::parse(&self.url)?;
        let host = parsed.host_str().unwrap_or("").to_string();
        let tls = parsed
            .query_pairs()
            .find(|(key, _)| key == "tls")
            .map(|(_, value)| value.into_owned());
        let parameters = TlsParameters::builder(host)
            .add_root_certificate(roots)
            .build()
            .context("the mail server's TLS, with its CA file")?;
        Ok(match (parsed.scheme(), tls.as_deref()) {
            ("smtps", _) => Tls::Wrapper(parameters),
            (_, Some("required")) => Tls::Required(parameters),
            (_, Some("opportunistic")) => Tls::Opportunistic(parameters),
            _ => Tls::None,
        })
    }

    /// The password from a mounted secret, `file:/path`, kept there.
    pub fn with_mounted(self, reference: &str) -> Result<Self> {
        let path = reference_path(reference)?;
        Ok(Self {
            password: Some(read_mounted(&path)?),
            mounted: Some(path),
            ..self
        })
    }

    /// Checks what can be checked without sending: the URL, and the from
    /// address. A refusal names which.
    pub fn check(&self) -> Result<()> {
        AsyncSmtpTransport::<Tokio1Executor>::from_url(&self.url).with_context(|| {
            format!(
                "`{}` is not a mail server's URL: smtps://host:465, or smtp://host:587?tls=required",
                self.url
            )
        })?;
        // Read the way lettre reads it (`url`, the first `tls`), so what is
        // checked is what will be sent.
        let url = self.url.as_str();
        let parsed = reqwest::Url::parse(url)?;
        let tls = parsed
            .query_pairs()
            .find(|(key, _)| key == "tls")
            .map(|(_, value)| value.into_owned());
        // Encrypted is `smtps://`, or `smtp://` with `tls=required`. With no
        // `tls` it is the clear, and `tls=opportunistic` is the clear whenever
        // the server offers no STARTTLS — which anyone between the two can
        // arrange. Either is the password and every code readable on the way:
        // only to this machine, a local relay.
        let encrypted = parsed.scheme() == "smtps" || tls.as_deref() == Some("required");
        let host = parsed.host_str().unwrap_or("");
        let local = host == "localhost"
            || host
                .trim_matches(['[', ']'])
                .parse::<std::net::IpAddr>()
                .is_ok_and(|ip| ip.is_loopback());
        if !encrypted && !local {
            anyhow::bail!(
                "`{url}` can send the password and every code unencrypted: use \
                 smtps://host:465, or smtp://host:587?tls=required"
            );
        }
        if parsed.password().is_some() {
            anyhow::bail!(
                "the password goes in its own field, not in the URL: the URL is written to \
                 config.toml, and the password to a file only its owner can read"
            );
        }
        self.from
            .parse::<lettre::message::Mailbox>()
            .with_context(|| format!("`{}` is not an address to send from", self.from))?;
        self.extra_roots()?;
        Ok(())
    }

    /// Sends one plain-text email.
    pub async fn send(&self, to: &str, subject: &str, body: &str) -> Result<()> {
        self.check()?;
        let mut transport = AsyncSmtpTransport::<Tokio1Executor>::from_url(&self.url)?
            .timeout(Some(Duration::from_secs(20)));
        if let Some(roots) = self.extra_roots()? {
            transport = transport.tls(self.tls_with(roots)?);
        }
        if let (Some(user), Some(password)) = (&self.user, &self.password) {
            transport = transport.credentials(Credentials::new(user.clone(), password.clone()));
        }
        let message = Message::builder()
            .from(self.from.parse()?)
            .to(to
                .parse()
                .with_context(|| format!("`{to}` is not an address"))?)
            .subject(subject)
            .header(ContentType::TEXT_PLAIN)
            .body(body.to_string())?;
        transport
            .build()
            .send(message)
            .await
            .with_context(|| format!("sending through {}", self.url))?;
        Ok(())
    }

    /// Sends `code` to `to`, as the login email says it.
    pub async fn send_code(&self, to: &str, code: &str, origin: Option<&str>) -> Result<()> {
        let (subject, body) = code_message(code, origin);
        self.send(to, &subject, &body).await
    }

    /// Writes the settings: the password into `<dir>/smtp.password`, `0600`,
    /// and the table that names it — or, for a mounted one, only the table,
    /// naming where it is mounted. Called once a code sent under them has
    /// come back.
    pub fn save(&self, dir: &Path) -> Result<SmtpTable> {
        let (password_file, password_ref) = match (&self.mounted, &self.password) {
            (Some(path), _) => {
                // A password luu wrote for earlier settings is not left behind.
                match std::fs::remove_file(dir.join(PASSWORD_FILE)) {
                    Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                        return Err(error).context("removing the earlier smtp.password");
                    }
                    _ => {}
                }
                (None, Some(format!("file:{}", path.display())))
            }
            (None, None) => (None, None),
            (None, Some(password)) => {
                let path = dir.join(PASSWORD_FILE);
                crate::secret::write(&path, password)?;
                (Some(path), None)
            }
        };
        Ok(SmtpTable {
            url: self.url.clone(),
            from: self.from.clone(),
            user: self.user.clone(),
            password_file,
            password_ref,
            ca_file: self.ca_file.clone(),
        })
    }
}

/// The file a `password-ref` names: `file:` and an absolute path.
///
/// The refusals never repeat what was written: a reference typed wrong may be
/// the password itself.
pub fn reference_path(reference: &str) -> Result<PathBuf> {
    let reference = reference.trim();
    if reference.starts_with("env:") {
        anyhow::bail!(
            "luu does not read the SMTP password from its environment: every command a session \
             runs inherits it, and on a public server those are other people's commands. Mount \
             it as a file and name it file:/path"
        );
    }
    let Some(path) = reference.strip_prefix("file:") else {
        anyhow::bail!(
            "a password reference is file:/path, a file mounted for luu, such as \
             file:/run/secrets/luu/smtp-password; the password itself is typed, or given in \
             --smtp-password-file"
        );
    };
    let path = PathBuf::from(path);
    if !path.is_absolute() {
        anyhow::bail!("file: needs an absolute path, such as file:/run/secrets/luu/smtp-password");
    }
    Ok(path)
}

/// A password from a file mounted for luu: a Kubernetes Secret, a Docker
/// secret. Read on every send, so a rotated secret is the next email's.
///
/// **Its mode is not checked.** A Secret is mounted `0644`, or `0440` under
/// `fsGroup`, and the cluster gave it that: inside luu's container there is
/// nobody else for the mode to keep out. What can read it there are the
/// commands sessions run — luu's own user, so no mode would stop them — and
/// [`refuse_where_commands_read`] is what does.
fn read_mounted(path: &Path) -> Result<String> {
    refuse_where_commands_read(path)?;
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("reading the SMTP password from {}", path.display()))?;
    // Its last line break is not part of it: `echo` and an editor add one.
    let password = text.trim_end_matches(['\r', '\n']);
    if password.is_empty() {
        anyhow::bail!("{} is empty", path.display());
    }
    Ok(password.to_string())
}

/// Refuses a password file under a root every command is granted to read
/// ([`agent_core::sandbox::SYSTEM_ROOTS`], `/etc` among them). The commands
/// run as luu's own user, so the file's mode does not keep them out; only
/// where it is does.
fn refuse_where_commands_read(path: &Path) -> Result<()> {
    let real = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    for root in agent_core::sandbox::SYSTEM_ROOTS {
        let root = Path::new(root);
        let real_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
        let under = [path, real.as_path()]
            .iter()
            .any(|path| path.starts_with(root) || path.starts_with(&real_root));
        if under {
            anyhow::bail!(
                "{}: every command a session runs can read {}, and runs as luu's own user. Keep \
                 the SMTP password elsewhere: in luu's state directory, or a secret mounted \
                 under /run",
                path.display(),
                root.display()
            );
        }
    }
    Ok(())
}

/// The login email: the code first, where a phone's preview shows it.
pub fn code_message(code: &str, origin: Option<&str>) -> (String, String) {
    let subject = format!("Your luu code: {code}");
    let place = origin
        .map(|origin| format!(" at {origin}"))
        .unwrap_or_default();
    let body = format!(
        "{code}\n\nYour code to log in to luu{place}. It lasts {} minutes and works once.\n\n\
         If you did not ask for it, nothing happens: without the code nobody logs in.\n",
        crate::users::CODE_MINUTES
    );
    (subject, body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_url_and_a_from_are_checked_before_anything_is_sent() {
        let good = Mail {
            url: "smtps://smtp.example.com:465".into(),
            from: "luu <luu@example.com>".into(),
            user: Some("luu".into()),
            password: Some("p".into()),
            mounted: None,
            ca_file: None,
        };
        assert!(good.check().is_ok());
        let starttls = Mail {
            url: "smtp://smtp.example.com:587?tls=required".into(),
            ..good.clone()
        };
        assert!(starttls.check().is_ok());
        let bad_url = Mail {
            url: "http://example.com".into(),
            ..good.clone()
        };
        assert!(bad_url.check().is_err());
        // The clear, only to this machine.
        let clear = Mail {
            url: "smtp://smtp.example.com:25".into(),
            ..good.clone()
        };
        assert!(
            clear
                .check()
                .unwrap_err()
                .to_string()
                .contains("unencrypted")
        );
        let relay = Mail {
            url: "smtp://127.0.0.1:2525".into(),
            ..good.clone()
        };
        assert!(relay.check().is_ok());
        // `opportunistic` is the clear whenever STARTTLS is not offered: found
        // against Mailpit on 2026-11-04, where it sent the password in the
        // clear to a host that was not this machine.
        for url in [
            "smtp://smtp.example.com:587?tls=opportunistic",
            "smtp://smtp.example.com:587?tls=opportunistic&tls=required",
            "smtp://smtp.example.com:587#?tls=required",
        ] {
            let mail = Mail {
                url: url.into(),
                ..good.clone()
            };
            assert!(
                mail.check()
                    .is_err_and(|e| e.to_string().contains("unencrypted")),
                "{url}"
            );
        }
        for url in [
            "smtp://smtp.example.com:587?tls=required",
            "smtp://smtp.example.com:587?x=1&tls=required",
            "smtps://smtp.example.com",
            "smtp://localhost:1025",
            "smtp://127.0.0.2:1025?tls=opportunistic",
            "smtp://[::1]:1025",
        ] {
            let mail = Mail {
                url: url.into(),
                ..good.clone()
            };
            assert!(mail.check().is_ok(), "{url}: {:?}", mail.check());
        }
        let inline = Mail {
            url: "smtps://luu:secret@smtp.example.com:465".into(),
            ..good.clone()
        };
        assert!(
            inline
                .check()
                .unwrap_err()
                .to_string()
                .contains("own field")
        );
        let bad_from = Mail {
            from: "not an address".into(),
            ..good
        };
        assert!(
            bad_from
                .check()
                .unwrap_err()
                .to_string()
                .contains("send from")
        );
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("luu-mail-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a scratch folder");
        dir
    }

    fn table(password_file: Option<PathBuf>, password_ref: Option<String>) -> SmtpTable {
        SmtpTable {
            url: "smtps://smtp.example.com:465".into(),
            from: "luu <luu@example.com>".into(),
            user: Some("luu".into()),
            password_file,
            password_ref,
            ca_file: None,
        }
    }

    /// A CA file is read before anything is sent, and one that cannot be —
    /// not there, relative, not PEM — is a refusal naming it, never a send
    /// under the public roots alone. It is kept in the table, and may sit
    /// where a password may not: a certificate is public.
    #[test]
    fn a_ca_file_is_read_before_a_send_and_kept_in_the_table() {
        let dir = scratch("ca");
        let good = Mail {
            url: "smtps://mail.internal:465".into(),
            from: "luu <luu@example.com>".into(),
            ..Mail::default()
        };
        let refused = |path: &str| {
            good.clone()
                .with_ca_file(Some(PathBuf::from(path)))
                .check()
                .map(|()| String::new())
                .unwrap_or_else(|error| format!("{error:#}"))
        };
        let missing = dir.join("nope.pem");
        assert!(refused(missing.to_str().unwrap()).contains("nope.pem"));
        assert!(refused("ca.pem").contains("not an absolute path"));
        let text = dir.join("notes.txt");
        std::fs::write(&text, "not a certificate\n").unwrap();
        assert!(refused(text.to_str().unwrap()).contains("holds no certificate"));
        let broken = dir.join("broken.pem");
        std::fs::write(
            &broken,
            "-----BEGIN CERTIFICATE-----\n!!!\n-----END CERTIFICATE-----\n",
        )
        .unwrap();
        assert!(refused(broken.to_str().unwrap()).contains("broken.pem"));

        // In the table as written, and back out of it.
        let with = good.clone().with_ca_file(Some(missing.clone()));
        let saved = with.save(&dir).unwrap();
        assert_eq!(saved.ca_file.as_deref(), Some(missing.as_path()));
        assert_eq!(Mail::from_table(&saved).unwrap().ca_file, Some(missing));
        // `/etc` is refused for a password, and is where a CA usually is.
        let under_etc = SmtpTable {
            ca_file: Some(PathBuf::from("/etc/ssl/private-ca.pem")),
            ..table(None, None)
        };
        assert!(Mail::from_table(&under_etc).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_reference_is_a_file_never_the_environment_and_never_echoed() {
        let env = reference_path("env:SMTP_PASSWORD").unwrap_err().to_string();
        assert!(env.contains("inherits"), "{env}");
        let typed = reference_path("hunter2-the-password")
            .unwrap_err()
            .to_string();
        assert!(typed.contains("file:/path"), "{typed}");
        assert!(!typed.contains("hunter2"), "{typed}");
        assert!(reference_path("file:secrets/smtp").is_err());
        assert_eq!(
            reference_path(" file:/run/secrets/luu/smtp-password ").unwrap(),
            PathBuf::from("/run/secrets/luu/smtp-password")
        );
    }

    #[test]
    fn a_mounted_password_is_read_as_mounted_and_saved_as_a_reference() {
        use std::os::unix::fs::PermissionsExt;

        let dir = scratch("mounted");
        let secret = dir.join("smtp-password");
        std::fs::write(&secret, "s3cret\n").unwrap();
        // What a Kubernetes Secret is mounted with.
        std::fs::set_permissions(&secret, std::fs::Permissions::from_mode(0o644)).unwrap();
        let reference = format!("file:{}", secret.display());
        let mail = Mail::from_table(&table(None, Some(reference.clone()))).unwrap();
        assert_eq!(mail.password.as_deref(), Some("s3cret"));
        assert_eq!(mail.mounted.as_deref(), Some(secret.as_path()));

        // Saved, it is named and not copied, and the one luu wrote before is
        // gone.
        let state = dir.join("state");
        crate::secret::write(&state.join(PASSWORD_FILE), "earlier").unwrap();
        let saved = mail.save(&state).unwrap();
        assert_eq!(saved.password_ref.as_deref(), Some(reference.as_str()));
        assert_eq!(saved.password_file, None);
        assert!(!state.join(PASSWORD_FILE).exists());

        // A rotated secret is the next read's.
        std::fs::write(&secret, "rotated").unwrap();
        let again = Mail::from_table(&saved).unwrap();
        assert_eq!(again.password.as_deref(), Some("rotated"));

        // Both at once is a mistake, said.
        let both = table(Some(dir.join("x")), Some(reference));
        assert!(
            Mail::from_table(&both)
                .unwrap_err()
                .to_string()
                .contains("twice")
        );

        std::fs::write(&secret, "\n").unwrap();
        assert!(
            Mail::from_table(&saved)
                .unwrap_err()
                .to_string()
                .contains("empty")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_password_file_where_every_command_reads_is_refused() {
        for table in [
            table(None, Some("file:/etc/luu/smtp-password".into())),
            table(Some(PathBuf::from("/etc/luu/smtp.password")), None),
            table(None, Some("file:/usr/local/share/smtp".into())),
        ] {
            let refused = Mail::from_table(&table).unwrap_err().to_string();
            assert!(refused.contains("every command"), "{refused}");
        }
        let mail = Mail::default()
            .with_mounted("file:/etc/smtp-password")
            .unwrap_err()
            .to_string();
        assert!(mail.contains("every command"), "{mail}");
    }

    #[test]
    fn the_code_is_in_the_subject_where_a_preview_shows_it() {
        let (subject, body) = code_message("123456", Some("https://luu.example.com"));
        assert_eq!(subject, "Your luu code: 123456");
        assert!(body.starts_with("123456\n"));
        assert!(body.contains("https://luu.example.com"));
    }
}
