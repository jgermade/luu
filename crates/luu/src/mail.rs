//! Email: the code a user logs in with. The only mail luu sends.
//!
//! **Settings are saved only once they have worked.** The first code — the
//! owner's — goes out under the settings as typed, and they are written to
//! `[auth.smtp]` and `smtp.password` only when that code comes back. So the
//! code is the test of the mail and of the mailbox at once. See
//! `RECORD/2026-11-03.an-owner-proved-by-email.WIP.md`.

use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
use lettre::message::header::ContentType;
use lettre::transport::smtp::authentication::Credentials;
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};

use crate::provider::SmtpTable;

/// Mail settings with the password read, ready to send.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mail {
    pub url: String,
    pub from: String,
    pub user: Option<String>,
    pub password: Option<String>,
}

/// Where the password goes, in the state directory.
pub const PASSWORD_FILE: &str = "smtp.password";

impl Mail {
    /// The settings `config.toml` has, the password read from its file.
    pub fn from_table(table: &SmtpTable) -> Result<Self> {
        let password = match &table.password_file {
            None => None,
            Some(path) => Some(crate::secret::read(
                &crate::provider::expand_home(path),
                "the SMTP password file",
                "hands the mail account to anyone who can read it",
            )?),
        };
        Ok(Self {
            url: table.url.clone(),
            from: table.from.clone(),
            user: table.user.clone(),
            password,
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
        // `smtp://` with no `tls=` is the password and the code in the clear:
        // only to this machine, a local relay.
        let url = self.url.as_str();
        let plain = url.starts_with("smtp://") && !url.contains("tls=");
        let host = url
            .trim_start_matches("smtp://")
            .split(['/', '?'])
            .next()
            .unwrap_or("");
        let host = host.rsplit_once('@').map_or(host, |(_, host)| host);
        let host = host.rsplit_once(':').map_or(host, |(host, _)| host);
        let local = matches!(host, "localhost" | "127.0.0.1" | "[::1]" | "::1");
        if plain && !local {
            anyhow::bail!(
                "`{url}` sends the password and every code unencrypted: use smtps://host:465, \
                 or smtp://host:587?tls=required"
            );
        }
        if url.contains('@') && url.split('@').next().is_some_and(|auth| auth.contains(':')) {
            anyhow::bail!(
                "the password goes in its own field, not in the URL: the URL is written to \
                 config.toml, and the password to a file only its owner can read"
            );
        }
        self.from
            .parse::<lettre::message::Mailbox>()
            .with_context(|| format!("`{}` is not an address to send from", self.from))?;
        Ok(())
    }

    /// Sends one plain-text email.
    pub async fn send(&self, to: &str, subject: &str, body: &str) -> Result<()> {
        self.check()?;
        let mut transport = AsyncSmtpTransport::<Tokio1Executor>::from_url(&self.url)?
            .timeout(Some(Duration::from_secs(20)));
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
    /// and the table that names it. Called once a code sent under them has
    /// come back.
    pub fn save(&self, dir: &Path) -> Result<SmtpTable> {
        let password_file = match &self.password {
            None => None,
            Some(password) => {
                let path = dir.join(PASSWORD_FILE);
                crate::secret::write(&path, password)?;
                Some(path)
            }
        };
        Ok(SmtpTable {
            url: self.url.clone(),
            from: self.from.clone(),
            user: self.user.clone(),
            password_file,
        })
    }
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

    #[test]
    fn the_code_is_in_the_subject_where_a_preview_shows_it() {
        let (subject, body) = code_message("123456", Some("https://luu.example.com"));
        assert_eq!(subject, "Your luu code: 123456");
        assert!(body.starts_with("123456\n"));
        assert!(body.contains("https://luu.example.com"));
    }
}
