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
/// How long a code by email waits before another to the same address.
pub const CODE_COOLDOWN_SECONDS: u64 = 60;
/// How long a setup link lasts.
pub const SETUP_MINUTES: u64 = 60;

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

/// A key on a user's account: what links their luus to this one.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Key {
    /// `SHA256:…`, as `ssh-keygen -l` prints it: how it is named on the wire.
    pub fingerprint: String,
    /// What the person calls it: the key's comment, unless they said.
    pub name: String,
    /// `ssh-ed25519 AAAA… comment`.
    pub public: String,
    /// Milliseconds since the epoch.
    pub added_at: u64,
    pub last_used: Option<u64>,
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

/// 2 added `codes.issued_at`, for one code by email a minute, and
/// `setup_keys`, for the one-time link that makes the first owner in the
/// page. See `RECORD/2026-11-03.an-owner-proved-by-email.WIP.md`.
/// 3 added `keys`: a user's SSH keys, which link their luus to this one. See
/// `RECORD/2026-11-03.one-luu-to-another-over-ssh.WIP.md`.
const SCHEMA: i32 = 3;

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
    tries      INTEGER NOT NULL,
    issued_at  INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS logins (
    hash       BLOB PRIMARY KEY,
    user       TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS setup_keys (
    hash       BLOB PRIMARY KEY,
    expires_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS keys (
    fingerprint TEXT PRIMARY KEY,
    user        TEXT NOT NULL,
    name        TEXT NOT NULL,
    public      TEXT NOT NULL,
    added_at    INTEGER NOT NULL,
    last_used   INTEGER
);";

/// Schema 2 to 3: the table, no row touched.
const TO_3: &str = "
CREATE TABLE IF NOT EXISTS keys (
    fingerprint TEXT PRIMARY KEY,
    user        TEXT NOT NULL,
    name        TEXT NOT NULL,
    public      TEXT NOT NULL,
    added_at    INTEGER NOT NULL,
    last_used   INTEGER
);";

/// Schema 1 to 2: the column and the table, no row touched.
const TO_2: &str = "
ALTER TABLE codes ADD COLUMN issued_at INTEGER NOT NULL DEFAULT 0;
CREATE TABLE IF NOT EXISTS setup_keys (
    hash       BLOB PRIMARY KEY,
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
            1 => {
                connection.execute_batch(TO_2)?;
                connection.execute_batch(TO_3)?;
                connection.pragma_update(None, "user_version", SCHEMA)?;
            }
            2 => {
                connection.execute_batch(TO_3)?;
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
        connection.execute("DELETE FROM keys WHERE user = ?1", [&user.id])?;
        connection.execute("DELETE FROM users WHERE id = ?1", [&user.id])?;
        Ok(user)
    }

    /// A key that links this user's luus here: `ssh-ed25519 AAAA… name`. A
    /// key belongs to one user, so the same key twice is refused, whoever
    /// added it first.
    pub fn add_key(&self, user_id: &str, name: &str, public: &str) -> Result<Key> {
        let key = crate::link::parse_public(public)?;
        let fingerprint = crate::link::fingerprint(&key);
        let name = match name.trim() {
            "" => key.comment().to_string(),
            name => name.to_string(),
        };
        let name = match name.is_empty() {
            true => fingerprint.clone(),
            false => name.chars().take(64).collect(),
        };
        let connection = self.connection();
        if self.get_on(&connection, user_id)?.is_none() {
            bail!("no such user");
        }
        let taken: Option<String> = connection
            .query_row(
                "SELECT user FROM keys WHERE fingerprint = ?1",
                [&fingerprint],
                |row| row.get(0),
            )
            .optional()?;
        if taken.is_some() {
            bail!("{fingerprint} is already on an account here");
        }
        let added = Key {
            fingerprint,
            name,
            public: crate::link::public_line(&key),
            added_at: crate::session::now_ms(),
            last_used: None,
        };
        connection.execute(
            "INSERT INTO keys (fingerprint, user, name, public, added_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![
                added.fingerprint,
                user_id,
                added.name,
                added.public,
                added.added_at as i64
            ],
        )?;
        Ok(added)
    }

    /// A user's keys, oldest first.
    pub fn keys_of(&self, user_id: &str) -> Result<Vec<Key>> {
        let connection = self.connection();
        let mut statement = connection.prepare(
            "SELECT fingerprint, name, public, added_at, last_used FROM keys
             WHERE user = ?1 ORDER BY added_at",
        )?;
        let rows = statement.query_map([user_id], |row| {
            Ok(Key {
                fingerprint: row.get(0)?,
                name: row.get(1)?,
                public: row.get(2)?,
                added_at: row.get::<_, i64>(3)? as u64,
                last_used: row.get::<_, Option<i64>>(4)?.map(|at| at as u64),
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// Takes a key off a user's account, by its fingerprint or its name.
    /// `false` when they had none such.
    pub fn remove_key(&self, user_id: &str, which: &str) -> Result<bool> {
        let removed = self.connection().execute(
            "DELETE FROM keys WHERE user = ?1 AND (fingerprint = ?2 OR name = ?2)",
            [user_id, which],
        )?;
        Ok(removed > 0)
    }

    /// Whose a key is, by its fingerprint: a user who can log in, or nobody.
    /// What a link is let in by, and checked again while it is open.
    pub fn key_user(&self, fingerprint: &str) -> Result<Option<User>> {
        let connection = self.connection();
        let user: Option<String> = connection
            .query_row(
                "SELECT user FROM keys WHERE fingerprint = ?1",
                [fingerprint],
                |row| row.get(0),
            )
            .optional()?;
        let Some(user) = user else {
            return Ok(None);
        };
        let user = self.get_on(&connection, &user)?;
        if user.as_ref().is_some_and(|user| !user.suspended) {
            connection.execute(
                "UPDATE keys SET last_used = ?1 WHERE fingerprint = ?2",
                rusqlite::params![crate::session::now_ms() as i64, fingerprint],
            )?;
        }
        Ok(user.filter(|user| !user.suspended))
    }

    fn get_on(&self, connection: &Connection, id: &str) -> Result<Option<User>> {
        Ok(connection
            .query_row(
                "SELECT id, email, role, suspended, created_at, last_login FROM users WHERE id = ?1",
                [id],
                user_of,
            )
            .optional()?)
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
        store_code(&connection, &user.email)
    }

    /// A code to email to `email`, for `POST /api/auth/code`: only for a user
    /// who exists and is not suspended, and not a second within
    /// [`CODE_COOLDOWN_SECONDS`]. `None` for every reason not to send one,
    /// which the caller answers the same way whatever it was.
    pub fn code_to_mail(&self, email: &str) -> Result<Option<String>> {
        let Ok(email) = email_of(email) else {
            return Ok(None);
        };
        let connection = self.connection();
        if !find(&connection, &email)?.is_some_and(|user| !user.suspended)
            || cooling(&connection, &email)?
        {
            return Ok(None);
        }
        Ok(Some(store_code(&connection, &email)?))
    }

    /// A code to email to whoever is to be the first owner: for an address
    /// that need not be a user yet, only while the server has no owner, and
    /// under the same minute's rule.
    pub fn code_to_claim(&self, email: &str) -> Result<Option<String>> {
        let email = email_of(email)?;
        let connection = self.connection();
        if has_owner(&connection)? || cooling(&connection, &email)? {
            return Ok(None);
        }
        Ok(Some(store_code(&connection, &email)?))
    }

    /// The first owner, made by the code their address was sent, and logged
    /// in. `None` when the code is not right, or when the server already has
    /// an owner. A user of that address already there is made the owner; any
    /// setup link still waiting is spent.
    pub fn claim_owner(
        &self,
        email: &str,
        code: &str,
        hours: u64,
    ) -> Result<Option<(User, String)>> {
        let Ok(email) = email_of(email) else {
            return Ok(None);
        };
        let connection = self.connection();
        if has_owner(&connection)? || !spend_code(&connection, &email, code)? {
            return Ok(None);
        }
        let user = match find(&connection, &email)? {
            Some(user) => {
                connection.execute(
                    "UPDATE users SET role = 'owner', suspended = 0 WHERE id = ?1",
                    [&user.id],
                )?;
                User {
                    role: Role::Owner,
                    suspended: false,
                    ..user
                }
            }
            None => {
                let user = User {
                    id: hex(&random::<6>()?),
                    email: email.clone(),
                    role: Role::Owner,
                    suspended: false,
                    created_at: crate::session::now_ms(),
                    last_login: None,
                };
                connection.execute(
                    "INSERT INTO users (id, email, role, suspended, created_at)
                     VALUES (?1, ?2, 'owner', 0, ?3)",
                    rusqlite::params![user.id, user.email, user.created_at as i64],
                )?;
                user
            }
        };
        connection.execute("DELETE FROM setup_keys", [])?;
        let token = login(&connection, &user, hours)?;
        Ok(Some((user, token)))
    }

    /// Spends `code` if it is the one `email` was given: what proves new mail
    /// settings to an owner who already exists.
    pub fn check_code(&self, email: &str, code: &str) -> Result<bool> {
        let Ok(email) = email_of(email) else {
            return Ok(false);
        };
        spend_code(&self.connection(), &email, code)
    }

    /// Forgets the code waiting for `email`: one that could not be sent, so
    /// it does not hold the minute's rule against a second try.
    pub fn forget_code(&self, email: &str) -> Result<()> {
        if let Ok(email) = email_of(email) {
            self.connection()
                .execute("DELETE FROM codes WHERE email = ?1", [email])?;
        }
        Ok(())
    }

    /// Every owner who can log in.
    pub fn owners(&self) -> Result<Vec<User>> {
        Ok(self
            .list()?
            .into_iter()
            .filter(|user| user.role == Role::Owner && !user.suspended)
            .collect())
    }

    /// Whether anybody can log in as an owner.
    pub fn has_owner(&self) -> Result<bool> {
        has_owner(&self.connection())
    }

    /// A one-time setup link's key: 32 random bytes, valid for
    /// [`SETUP_MINUTES`], spent when an owner is made. Only while there is no
    /// owner: afterwards an owner adds people.
    pub fn issue_setup_key(&self) -> Result<String> {
        let connection = self.connection();
        if has_owner(&connection)? {
            bail!("this server has an owner: they add people, and no setup link is needed");
        }
        let key = hex(&random::<32>()?);
        let expires_at = crate::session::now_ms() + SETUP_MINUTES * 60_000;
        connection.execute(
            "INSERT INTO setup_keys (hash, expires_at) VALUES (?1, ?2)",
            rusqlite::params![login_hash(&key), expires_at as i64],
        )?;
        Ok(key)
    }

    /// Whether `key` opens setup: issued, not expired, and no owner yet.
    pub fn setup_key_opens(&self, key: &str) -> Result<bool> {
        let connection = self.connection();
        if has_owner(&connection)? {
            return Ok(false);
        }
        let now = crate::session::now_ms() as i64;
        connection.execute("DELETE FROM setup_keys WHERE expires_at <= ?1", [now])?;
        let found: Option<i64> = connection
            .query_row(
                "SELECT expires_at FROM setup_keys WHERE hash = ?1",
                [login_hash(key)],
                |row| row.get(0),
            )
            .optional()?;
        Ok(found.is_some())
    }

    /// A code exchanged for a login: the user and the cookie's value, or
    /// `None` for every way it can fail, which a caller answers the same way
    /// whatever the reason.
    pub fn redeem(&self, email: &str, code: &str, hours: u64) -> Result<Option<(User, String)>> {
        let Ok(email) = email_of(email) else {
            return Ok(None);
        };
        let connection = self.connection();
        if !spend_code(&connection, &email, code)? {
            return Ok(None);
        }
        let Some(user) = find(&connection, &email)?.filter(|user| !user.suspended) else {
            return Ok(None);
        };
        let token = login(&connection, &user, hours)?;
        Ok(Some((
            User {
                last_login: Some(crate::session::now_ms()),
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

/// A new code for `email`, replacing any it had, timed for the minute's rule.
fn store_code(connection: &Connection, email: &str) -> Result<String> {
    let code = six_digits()?;
    let now = crate::session::now_ms();
    connection.execute(
        "INSERT INTO codes (email, hash, expires_at, tries, issued_at) VALUES (?1, ?2, ?3, 0, ?4)
         ON CONFLICT(email) DO UPDATE SET
             hash = excluded.hash, expires_at = excluded.expires_at, tries = 0,
             issued_at = excluded.issued_at",
        rusqlite::params![
            email,
            code_hash(email, &code),
            (now + CODE_MINUTES * 60_000) as i64,
            now as i64
        ],
    )?;
    Ok(code)
}

/// Whether a code went to `email` less than a minute ago.
fn cooling(connection: &Connection, email: &str) -> Result<bool> {
    let issued: Option<i64> = connection
        .query_row(
            "SELECT issued_at FROM codes WHERE email = ?1",
            [email],
            |row| row.get(0),
        )
        .optional()?;
    let now = crate::session::now_ms() as i64;
    Ok(issued.is_some_and(|at| now - at < (CODE_COOLDOWN_SECONDS * 1000) as i64))
}

/// Checks `code` against the one `email` was given, and spends it: once
/// right, or after the tries run out, or once expired.
fn spend_code(connection: &Connection, email: &str, code: &str) -> Result<bool> {
    let code: String = code.chars().filter(|c| !c.is_whitespace()).collect();
    let now = crate::session::now_ms();
    let waiting: Option<(Vec<u8>, i64, u32)> = connection
        .query_row(
            "SELECT hash, expires_at, tries FROM codes WHERE email = ?1",
            [email],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    let Some((hash, expires_at, tries)) = waiting else {
        return Ok(false);
    };
    if (expires_at as u64) <= now || tries >= CODE_TRIES {
        connection.execute("DELETE FROM codes WHERE email = ?1", [email])?;
        return Ok(false);
    }
    if !same(&hash, &code_hash(email, &code)) {
        match tries + 1 >= CODE_TRIES {
            true => connection.execute("DELETE FROM codes WHERE email = ?1", [email])?,
            false => connection.execute(
                "UPDATE codes SET tries = tries + 1 WHERE email = ?1",
                [email],
            )?,
        };
        return Ok(false);
    }
    connection.execute("DELETE FROM codes WHERE email = ?1", [email])?;
    Ok(true)
}

/// A new login for `user`, for `hours`: the cookie's value.
fn login(connection: &Connection, user: &User, hours: u64) -> Result<String> {
    let now = crate::session::now_ms();
    connection.execute("DELETE FROM logins WHERE expires_at <= ?1", [now as i64])?;
    let token = hex(&random::<32>()?);
    connection.execute(
        "INSERT INTO logins (hash, user, created_at, expires_at) VALUES (?1, ?2, ?3, ?4)",
        rusqlite::params![
            login_hash(&token),
            user.id,
            now as i64,
            (now + hours.max(1) * 3_600_000) as i64
        ],
    )?;
    connection.execute(
        "UPDATE users SET last_login = ?2 WHERE id = ?1",
        rusqlite::params![user.id, now as i64],
    )?;
    Ok(token)
}

fn has_owner(connection: &Connection) -> Result<bool> {
    let owners: i64 = connection.query_row(
        "SELECT COUNT(*) FROM users WHERE role = 'owner' AND suspended = 0",
        [],
        |row| row.get(0),
    )?;
    Ok(owners > 0)
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

pub(crate) fn random<const N: usize>() -> Result<[u8; N]> {
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
    fn a_key_is_one_users_and_lets_in_only_while_they_can_log_in() {
        let (dir, users) = scratch("keys");
        let ana = users.add("ana@example.com", Role::Owner).unwrap();
        let bo = users.add("bo@example.com", Role::User).unwrap();
        let key = crate::link::identity(&dir).unwrap();
        let line = crate::link::public_line(key.public_key());
        let added = users.add_key(&bo.id, "", &line).unwrap();
        assert!(added.fingerprint.starts_with("SHA256:"));
        assert!(
            added.name.starts_with("luu@"),
            "the comment names it: {}",
            added.name
        );
        // One key, one user: the same key on a second account is refused.
        assert!(users.add_key(&ana.id, "casa", &line).is_err());
        assert!(
            users
                .add_key(&bo.id, "x", "ssh-rsa AAAAB3NzaC1yc2E x")
                .is_err()
        );
        assert_eq!(users.keys_of(&bo.id).unwrap().len(), 1);
        assert!(users.keys_of(&ana.id).unwrap().is_empty());
        let whose = users.key_user(&added.fingerprint).unwrap().unwrap();
        assert_eq!(whose.id, bo.id);
        assert!(users.keys_of(&bo.id).unwrap()[0].last_used.is_some());
        // Suspended, the key lets nobody in; resumed, it does again.
        users.set_suspended("bo@example.com", true).unwrap();
        assert!(users.key_user(&added.fingerprint).unwrap().is_none());
        users.set_suspended("bo@example.com", false).unwrap();
        assert!(users.key_user(&added.fingerprint).unwrap().is_some());
        // Taken off by name, it is gone; and a user removed takes theirs.
        assert!(users.remove_key(&bo.id, &added.name).unwrap());
        assert!(users.key_user(&added.fingerprint).unwrap().is_none());
        users.add_key(&bo.id, "casa", &line).unwrap();
        users.remove("bo@example.com").unwrap();
        assert!(users.key_user(&added.fingerprint).unwrap().is_none());
        assert!(users.add_key(&ana.id, "casa", &line).is_ok());
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

    #[test]
    fn a_setup_link_makes_one_owner_by_a_code_sent_to_them() {
        let (dir, users) = scratch("claim");
        assert!(!users.has_owner().unwrap());
        let key = users.issue_setup_key().unwrap();
        assert_eq!(key.len(), 64);
        assert!(users.setup_key_opens(&key).unwrap());
        assert!(!users.setup_key_opens("not-the-key").unwrap());

        let code = users
            .code_to_claim("Jo@Example.com")
            .unwrap()
            .expect("a code");
        // One a minute.
        assert!(users.code_to_claim("jo@example.com").unwrap().is_none());
        let wrong = if code == "000000" { "000001" } else { "000000" };
        assert!(
            users
                .claim_owner("jo@example.com", wrong, 1)
                .unwrap()
                .is_none()
        );
        let (owner, token) = users
            .claim_owner("jo@example.com", &code, 1)
            .unwrap()
            .expect("the owner");
        assert_eq!(owner.role, Role::Owner);
        assert_eq!(
            users.identify(&token).unwrap().map(|u| u.email),
            Some("jo@example.com".into())
        );
        // Spent: the link, and any second claim.
        assert!(!users.setup_key_opens(&key).unwrap());
        assert!(users.issue_setup_key().is_err());
        assert!(users.code_to_claim("eve@example.com").unwrap().is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_code_by_mail_goes_only_to_a_user_and_once_a_minute() {
        let (dir, users) = scratch("mail");
        users.add("al@example.com", Role::User).unwrap();
        assert!(users.code_to_mail("nobody@example.com").unwrap().is_none());
        assert!(users.code_to_mail("not an email").unwrap().is_none());
        let code = users
            .code_to_mail("al@example.com")
            .unwrap()
            .expect("a code");
        assert!(
            users.code_to_mail("al@example.com").unwrap().is_none(),
            "a minute between"
        );
        assert!(users.redeem("al@example.com", &code, 1).unwrap().is_some());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_schema_1_file_gains_the_minute_and_the_setup_keys() {
        let dir = std::env::temp_dir().join(format!("luu-users-v1-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("auth.db");
        create_private(&path).unwrap();
        {
            let connection = Connection::open(&path).unwrap();
            connection
                .execute_batch(
                    "CREATE TABLE users (id TEXT PRIMARY KEY, email TEXT NOT NULL UNIQUE,
                       role TEXT NOT NULL, suspended INTEGER NOT NULL DEFAULT 0,
                       created_at INTEGER NOT NULL, last_login INTEGER);
                     CREATE TABLE codes (email TEXT PRIMARY KEY, hash BLOB NOT NULL,
                       expires_at INTEGER NOT NULL, tries INTEGER NOT NULL);
                     CREATE TABLE logins (hash BLOB PRIMARY KEY, user TEXT NOT NULL,
                       created_at INTEGER NOT NULL, expires_at INTEGER NOT NULL);
                     INSERT INTO users VALUES ('abc', 'old@example.com', 'owner', 0, 1, NULL);",
                )
                .unwrap();
            connection.pragma_update(None, "user_version", 1).unwrap();
        }
        let users = Users::open(&path).unwrap();
        assert!(users.has_owner().unwrap());
        let code = users.issue_code("old@example.com").unwrap();
        assert!(users.redeem("old@example.com", &code, 1).unwrap().is_some());
        // Schema 3's table came with it.
        let old = users.find("old@example.com").unwrap().unwrap();
        assert!(users.keys_of(&old.id).unwrap().is_empty());
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
