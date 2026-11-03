//! Who may log in to a public `luu serve`, and with what.
//!
//! `auth.db`, beside `sessions.db` in the state directory: the users, the
//! one-time codes they log in with, and the logins those codes became. The
//! configuration says how the server is reached; this says *who* reaches it,
//! because with admins adding people from the page the list is written while
//! the server runs, and the person's own `config.toml` is the wrong file for
//! that. See `RECORD/2026-11-03.users-and-a-code-by-email.WIP.md` and, for
//! this file, `RECORD/2026-11-03.a-user-a-code-and-a-folder.WIP.md`.
//!
//! **Nothing secret is stored as itself.** A code is kept as the `sha256` of
//! the email and the code, and a login as the `sha256` of the cookie's value:
//! what this file holds lets a reader check a guess, not log in.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{Context, Result, bail};
use rusqlite::{Connection, OptionalExtension};
use sha2::{Digest, Sha256};

/// How long a code lasts, from when it was printed.
pub const CODE_MINUTES: u64 = 10;
/// How many wrong tries burn a code.
pub const CODE_TRIES: u32 = 5;
/// How long a login lasts when `[auth] session-hours` says nothing: a week.
pub const SESSION_HOURS: u64 = 168;
/// The cookie a login is carried in.
pub const COOKIE: &str = "luu_login";

/// What a user may do. Ordered, so `role >= Role::Admin` reads as it means.
/// See the parent record's *Three roles*.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    serde::Serialize,
    serde::Deserialize,
    clap::ValueEnum,
)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// Their own sessions, their own folder.
    User,
    /// And other people, and what is shared inside luu.
    Admin,
    /// And the machine: what loopback is on a laptop.
    Owner,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Admin => "admin",
            Self::Owner => "owner",
        }
    }

    fn parse(text: &str) -> Option<Self> {
        match text {
            "user" => Some(Self::User),
            "admin" => Some(Self::Admin),
            "owner" => Some(Self::Owner),
            _ => None,
        }
    }
}

/// One person.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct User {
    /// Twelve hex characters, random: what their folder is called and what a
    /// session's owner says. Never the email, which can change and should not
    /// be a directory name anybody can list.
    pub id: String,
    pub email: String,
    pub role: Role,
    pub suspended: bool,
    /// Milliseconds since the epoch.
    pub created_at: u64,
    pub last_login: Option<u64>,
}

/// `auth.db`, opened. One connection behind a lock: every statement here is a
/// lookup or a single-row write, and `luu user` on the same file from another
/// process waits on SQLite's own lock rather than on this one.
pub struct Users {
    connection: Mutex<Connection>,
    path: PathBuf,
}

/// Where `auth.db` lives when nobody says otherwise: the state directory,
/// beside `sessions.db`. `None` where there is none.
pub fn default_path() -> Option<PathBuf> {
    Some(crate::config::dir()?.join("auth.db"))
}

const SCHEMA: i32 = 1;

const TABLES: &str = "
CREATE TABLE IF NOT EXISTS users (
    id         TEXT PRIMARY KEY,
    email      TEXT NOT NULL UNIQUE,
    role       TEXT NOT NULL,
    suspended  INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL,
    last_login INTEGER
);
CREATE TABLE IF NOT EXISTS codes (
    email      TEXT PRIMARY KEY,
    hash       BLOB NOT NULL,
    expires_at INTEGER NOT NULL,
    tries      INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS logins (
    hash       BLOB PRIMARY KEY,
    user       TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL
);";

impl Users {
    /// Opens `auth.db`, creating it `0600` when it is not there.
    ///
    /// **Its mode is checked on every open**, as a token file's is: this is the
    /// first store in the state directory that holds anything a reader could
    /// use, and a file the group can read is refused rather than warned about.
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        create_private(path)?;
        crate::secret::check_mode(
            path,
            "the user database",
            "hands every login to anyone who can read it",
        )?;
        let connection =
            Connection::open(path).with_context(|| format!("opening {}", path.display()))?;
        // `luu user` and `serve` share the file; one waits for the other.
        connection.busy_timeout(std::time::Duration::from_secs(5))?;
        let version: i32 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
        match version {
            0 => {
                connection.execute_batch(TABLES)?;
                connection.pragma_update(None, "user_version", SCHEMA)?;
            }
            SCHEMA => {}
            other => bail!(
                "{} is user database schema {other}, and this build knows {SCHEMA}: \
                 it was written by a newer luu",
                path.display()
            ),
        }
        Ok(Self {
            connection: Mutex::new(connection),
            path: path.to_path_buf(),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn connection(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.connection
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Adds a person. What an admin's *Add* and `luu user add` both are: there
    /// is no other way to become a user.
    pub fn add(&self, email: &str, role: Role) -> Result<User> {
        let email = email_of(email)?;
        let connection = self.connection();
        if find(&connection, &email)?.is_some() {
            bail!("{email} is already a user");
        }
        let user = User {
            id: hex(&random::<6>()?),
            email,
            role,
            suspended: false,
            created_at: crate::session::now_ms(),
            last_login: None,
        };
        connection.execute(
            "INSERT INTO users (id, email, role, suspended, created_at) VALUES (?1, ?2, ?3, 0, ?4)",
            rusqlite::params![user.id, user.email, role.as_str(), user.created_at as i64],
        )?;
        Ok(user)
    }

    /// Everyone, oldest first.
    pub fn list(&self) -> Result<Vec<User>> {
        let connection = self.connection();
        let mut statement = connection.prepare(
            "SELECT id, email, role, suspended, created_at, last_login FROM users
             ORDER BY created_at, email",
        )?;
        let rows = statement.query_map([], user_of)?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// One person, by email.
    pub fn find(&self, email: &str) -> Result<Option<User>> {
        find(&self.connection(), &email_of(email)?)
    }

    /// One person, by id.
    pub fn get(&self, id: &str) -> Result<Option<User>> {
        Ok(self
            .connection()
            .query_row(
                "SELECT id, email, role, suspended, created_at, last_login FROM users WHERE id = ?1",
                [id],
                user_of,
            )
            .optional()?)
    }

    /// Changes what someone may do. The last owner who can log in is not
    /// demoted: a server always has somebody who can add the next person.
    pub fn set_role(&self, email: &str, role: Role) -> Result<User> {
        let connection = self.connection();
        let user = existing(&connection, email)?;
        if user.role == Role::Owner && role != Role::Owner {
            last_owner_guard(&connection, &user, "demoted")?;
        }
        connection.execute(
            "UPDATE users SET role = ?2 WHERE id = ?1",
            rusqlite::params![user.id, role.as_str()],
        )?;
        Ok(User { role, ..user })
    }

    /// Suspends someone, or lets them back in. Suspending ends every login
    /// they have and any code waiting for them.
    pub fn set_suspended(&self, email: &str, suspended: bool) -> Result<User> {
        let connection = self.connection();
        let user = existing(&connection, email)?;
        if suspended && user.role == Role::Owner {
            last_owner_guard(&connection, &user, "suspended")?;
        }
        connection.execute(
            "UPDATE users SET suspended = ?2 WHERE id = ?1",
            rusqlite::params![user.id, suspended],
        )?;
        if suspended {
            end_everything(&connection, &user)?;
        }
        Ok(User { suspended, ..user })
    }

    /// Removes someone, their logins and their code. Their sessions are the
    /// caller's to have refused about first: what removing does with them is
    /// still open, so `luu user remove` refuses while there are any.
    pub fn remove(&self, email: &str) -> Result<User> {
        let connection = self.connection();
        let user = existing(&connection, email)?;
        if user.role == Role::Owner {
            last_owner_guard(&connection, &user, "removed")?;
        }
        end_everything(&connection, &user)?;
        connection.execute("DELETE FROM users WHERE id = ?1", [&user.id])?;
        Ok(user)
    }

    /// A code for `email`, replacing any it had: six digits, ten minutes, one
    /// use. Only for someone who is a user and is not suspended — the
    /// difference is said here, on the machine, and never to a request.
    pub fn issue_code(&self, email: &str) -> Result<String> {
        let connection = self.connection();
        let user = existing(&connection, email)?;
        if user.suspended {
            bail!("{} is suspended: `luu user resume` them first", user.email);
        }
        let code = six_digits()?;
        let expires_at = crate::session::now_ms() + CODE_MINUTES * 60_000;
        connection.execute(
            "INSERT INTO codes (email, hash, expires_at, tries) VALUES (?1, ?2, ?3, 0)
             ON CONFLICT(email) DO UPDATE SET
                 hash = excluded.hash, expires_at = excluded.expires_at, tries = 0",
            rusqlite::params![user.email, code_hash(&user.email, &code), expires_at as i64],
        )?;
        Ok(code)
    }

    /// A code exchanged for a login: the user and the cookie's value, or
    /// `None` for every way it can fail, which a caller answers the same way
    /// whatever the reason.
    pub fn redeem(&self, email: &str, code: &str, hours: u64) -> Result<Option<(User, String)>> {
        let Ok(email) = email_of(email) else {
            return Ok(None);
        };
        let code: String = code.chars().filter(|c| !c.is_whitespace()).collect();
        let now = crate::session::now_ms();
        let connection = self.connection();
        connection.execute("DELETE FROM logins WHERE expires_at <= ?1", [now as i64])?;
        let waiting: Option<(Vec<u8>, i64, u32)> = connection
            .query_row(
                "SELECT hash, expires_at, tries FROM codes WHERE email = ?1",
                [&email],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?;
        let Some((hash, expires_at, tries)) = waiting else {
            return Ok(None);
        };
        if (expires_at as u64) <= now || tries >= CODE_TRIES {
            connection.execute("DELETE FROM codes WHERE email = ?1", [&email])?;
            return Ok(None);
        }
        if !same(&hash, &code_hash(&email, &code)) {
            match tries + 1 >= CODE_TRIES {
                true => connection.execute("DELETE FROM codes WHERE email = ?1", [&email])?,
                false => connection.execute(
                    "UPDATE codes SET tries = tries + 1 WHERE email = ?1",
                    [&email],
                )?,
            };
            return Ok(None);
        }
        connection.execute("DELETE FROM codes WHERE email = ?1", [&email])?;
        let Some(user) = find(&connection, &email)?.filter(|user| !user.suspended) else {
            return Ok(None);
        };
        let token = hex(&random::<32>()?);
        let hours = hours.max(1);
        connection.execute(
            "INSERT INTO logins (hash, user, created_at, expires_at) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![
                login_hash(&token),
                user.id,
                now as i64,
                (now + hours * 3_600_000) as i64
            ],
        )?;
        connection.execute(
            "UPDATE users SET last_login = ?2 WHERE id = ?1",
            rusqlite::params![user.id, now as i64],
        )?;
        Ok(Some((
            User {
                last_login: Some(now),
                ..user
            },
            token,
        )))
    }

    /// Who a cookie's value is: a login not expired, of a user not suspended.
    pub fn identify(&self, token: &str) -> Result<Option<User>> {
        Ok(self
            .connection()
            .query_row(
                "SELECT u.id, u.email, u.role, u.suspended, u.created_at, u.last_login
                 FROM logins l JOIN users u ON u.id = l.user
                 WHERE l.hash = ?1 AND l.expires_at > ?2 AND u.suspended = 0",
                rusqlite::params![login_hash(token), crate::session::now_ms() as i64],
                user_of,
            )
            .optional()?)
    }

    /// Ends one login.
    pub fn logout(&self, token: &str) -> Result<()> {
        self.connection()
            .execute("DELETE FROM logins WHERE hash = ?1", [login_hash(token)])?;
        Ok(())
    }
}

/// Creates the file `0600` when it is not there — with the mode, so there is
/// no moment it is anything else. One that is already there is left as it is,
/// and its mode is the caller's to check.
fn create_private(path: &Path) -> Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    match options.open(path) {
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(error) => Err(error).with_context(|| format!("creating {}", path.display())),
    }
}

fn user_of(row: &rusqlite::Row<'_>) -> rusqlite::Result<User> {
    let role: String = row.get(2)?;
    Ok(User {
        id: row.get(0)?,
        email: row.get(1)?,
        // A role this build does not know is the least it could mean.
        role: Role::parse(&role).unwrap_or(Role::User),
        suspended: row.get(3)?,
        created_at: row.get::<_, i64>(4)? as u64,
        last_login: row.get::<_, Option<i64>>(5)?.map(|at| at as u64),
    })
}

fn find(connection: &Connection, email: &str) -> Result<Option<User>> {
    Ok(connection
        .query_row(
            "SELECT id, email, role, suspended, created_at, last_login FROM users WHERE email = ?1",
            [email],
            user_of,
        )
        .optional()?)
}

fn existing(connection: &Connection, email: &str) -> Result<User> {
    let email = email_of(email)?;
    find(connection, &email)?.with_context(|| format!("{email} is not a user"))
}

/// Refuses to take away the last owner who can log in.
fn last_owner_guard(connection: &Connection, user: &User, what: &str) -> Result<()> {
    let others: i64 = connection.query_row(
        "SELECT COUNT(*) FROM users WHERE role = 'owner' AND suspended = 0 AND id <> ?1",
        [&user.id],
        |row| row.get(0),
    )?;
    if others == 0 && !user.suspended {
        bail!(
            "{} is the last owner, and cannot be {what}: make someone else an owner first \
             (`luu user role <email> owner`)",
            user.email
        );
    }
    Ok(())
}

fn end_everything(connection: &Connection, user: &User) -> Result<()> {
    connection.execute("DELETE FROM logins WHERE user = ?1", [&user.id])?;
    connection.execute("DELETE FROM codes WHERE email = ?1", [&user.email])?;
    Ok(())
}

/// An email as it is stored and compared: trimmed and lower-cased, and
/// refused when it could not be one.
pub fn email_of(text: &str) -> Result<String> {
    let email = text.trim().to_lowercase();
    let shaped = email.len() <= 254
        && !email.chars().any(char::is_whitespace)
        && email
            .split_once('@')
            .is_some_and(|(name, domain)| !name.is_empty() && !domain.is_empty());
    if !shaped {
        bail!("`{}` is not an email", text.trim());
    }
    Ok(email)
}

fn code_hash(email: &str, code: &str) -> Vec<u8> {
    let mut hasher = Sha256::new();
    hasher.update(email.as_bytes());
    hasher.update([0u8]);
    hasher.update(code.as_bytes());
    hasher.finalize().to_vec()
}

fn login_hash(token: &str) -> Vec<u8> {
    Sha256::digest(token.as_bytes()).to_vec()
}

/// Compares without leaking where two digests differ.
fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn random<const N: usize>() -> Result<[u8; N]> {
    use std::io::Read;
    let mut bytes = [0u8; N];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut file| file.read_exact(&mut bytes))
        .context("reading /dev/urandom")?;
    Ok(bytes)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Six digits, every one of the million equally likely: a draw above the
/// largest multiple of a million under 2³² is drawn again rather than folded
/// onto the low codes.
fn six_digits() -> Result<String> {
    const LIMIT: u32 = 4_294_000_000;
    loop {
        let draw = u32::from_le_bytes(random::<4>()?);
        if draw < LIMIT {
            return Ok(format!("{:06}", draw % 1_000_000));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> (PathBuf, Users) {
        let dir = std::env::temp_dir().join(format!("luu-users-{name}-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).expect("a scratch directory");
        let path = dir.join("auth.db");
        let users = Users::open(&path).expect("auth.db opens");
        (dir, users)
    }

    #[test]
    fn a_code_becomes_a_login_once() {
        let (dir, users) = scratch("code");
        let ana = users.add(" Ana@Example.com ", Role::User).unwrap();
        assert_eq!(ana.email, "ana@example.com");
        assert_eq!(ana.id.len(), 12);

        let code = users.issue_code("ana@example.com").unwrap();
        assert_eq!(code.len(), 6);
        let spaced = format!("{} {}", &code[..3], &code[3..]);
        let (user, token) = users
            .redeem("ANA@example.com", &spaced, 1)
            .unwrap()
            .expect("the code logs in");
        assert_eq!(user.id, ana.id);
        assert_eq!(users.identify(&token).unwrap().map(|u| u.id), Some(ana.id));
        // One use.
        assert!(users.redeem("ana@example.com", &code, 1).unwrap().is_none());

        users.logout(&token).unwrap();
        assert!(users.identify(&token).unwrap().is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn five_wrong_tries_burn_a_code() {
        let (dir, users) = scratch("tries");
        users.add("bo@example.com", Role::User).unwrap();
        let code = users.issue_code("bo@example.com").unwrap();
        let wrong = if code == "000000" { "000001" } else { "000000" };
        for _ in 0..CODE_TRIES {
            assert!(users.redeem("bo@example.com", wrong, 1).unwrap().is_none());
        }
        assert!(
            users.redeem("bo@example.com", &code, 1).unwrap().is_none(),
            "the right code after five wrong ones is too late"
        );
        // And nobody who is not a user gets anything, however asked.
        assert!(
            users
                .redeem("nobody@example.com", &code, 1)
                .unwrap()
                .is_none()
        );
        assert!(users.redeem("not an email", &code, 1).unwrap().is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn suspending_ends_logins_and_the_last_owner_stays() {
        let (dir, users) = scratch("owner");
        users.add("jo@example.com", Role::Owner).unwrap();
        let error = users.set_role("jo@example.com", Role::Admin).unwrap_err();
        assert!(error.to_string().contains("last owner"), "{error}");
        assert!(users.set_suspended("jo@example.com", true).is_err());
        assert!(users.remove("jo@example.com").is_err());

        users.add("al@example.com", Role::Owner).unwrap();
        let code = users.issue_code("jo@example.com").unwrap();
        let (_, token) = users.redeem("jo@example.com", &code, 1).unwrap().unwrap();
        users.set_suspended("jo@example.com", true).unwrap();
        assert!(
            users.identify(&token).unwrap().is_none(),
            "a suspension ends the login"
        );
        assert!(users.issue_code("jo@example.com").is_err());
        // Al is now the last owner who can log in.
        assert!(users.set_role("al@example.com", Role::User).is_err());
        users.set_suspended("jo@example.com", false).unwrap();
        users.set_role("al@example.com", Role::User).unwrap();
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn one_email_one_user() {
        let (dir, users) = scratch("unique");
        users.add("cy@example.com", Role::User).unwrap();
        assert!(users.add("CY@example.com", Role::Admin).is_err());
        assert!(users.add("not-an-email", Role::User).is_err());
        assert_eq!(users.list().unwrap().len(), 1);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[cfg(unix)]
    #[test]
    fn the_file_is_private_and_a_loose_one_is_refused() {
        use std::os::unix::fs::PermissionsExt;
        let (dir, users) = scratch("mode");
        let path = users.path().to_path_buf();
        drop(users);
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        let error = Users::open(&path).err().expect("refused");
        assert!(error.to_string().contains("chmod 600"), "{error}");
        std::fs::remove_dir_all(&dir).ok();
    }
}
