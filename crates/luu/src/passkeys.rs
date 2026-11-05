//! Passkeys: WebAuthn's two ceremonies, registering one on a logged-in
//! account and logging in with one, over `webauthn-rs` 0.6, which is RustCrypto
//! throughout and links no OpenSSL. The ceremonies' state is kept here, in
//! memory, for five minutes; what a passkey *is* is kept in `auth.db` by
//! [`crate::users`]. See `RECORD/2026-11-05.passkeys.completed.md`.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use base64::Engine;
use webauthn_rs::prelude::{
    AuthenticationResult, CreationChallengeResponse, DiscoverableAuthentication, Passkey,
    PasskeyRegistration, PublicKeyCredential, RegisterPublicKeyCredential,
    RequestChallengeResponse, Url, Uuid, Webauthn, WebauthnBuilder,
};

/// How long a ceremony waits for the browser.
const CEREMONY: Duration = Duration::from_secs(300);
/// How many logins may be waiting at once: a ceremony is started by anyone,
/// so this is what keeps them from filling memory.
const WAITING: usize = 1000;

pub struct Passkeys {
    webauthn: Webauthn,
    /// By user id: one registration at a time per person.
    registering: Mutex<HashMap<String, (PasskeyRegistration, Instant)>>,
    /// By a random id the page carries from start to finish.
    logging_in: Mutex<HashMap<String, (DiscoverableAuthentication, Instant)>>,
}

impl Passkeys {
    /// The relying party is `origin`: its host the RP id, and it the one
    /// origin a ceremony is accepted from.
    pub fn new(origin: &str) -> Result<Self> {
        let url = Url::parse(origin).with_context(|| format!("{origin} is not a URL"))?;
        let host = url
            .host_str()
            .with_context(|| format!("{origin} has no host"))?
            .to_string();
        let webauthn = WebauthnBuilder::new(&host, &url)
            .context("a relying party for this origin")?
            .rp_name("luu")
            .build()
            .context("a relying party for this origin")?;
        Ok(Self {
            webauthn,
            registering: Mutex::default(),
            logging_in: Mutex::default(),
        })
    }

    /// The options for the browser's `navigator.credentials.create`, for
    /// `user_id`, excluding the passkeys they already have.
    pub fn start_registration(
        &self,
        user_id: &str,
        email: &str,
        existing: &[String],
    ) -> Result<serde_json::Value> {
        let exclude = existing
            .iter()
            .filter_map(|body| serde_json::from_str::<Passkey>(body).ok())
            .map(|passkey| passkey.cred_id().clone())
            .collect::<Vec<_>>();
        let (options, state) = self
            .webauthn
            .start_passkey_registration(handle(user_id), email, email, Some(exclude))
            .context("starting a passkey registration")?;
        let mut options: serde_json::Value =
            serde_json::to_value::<CreationChallengeResponse>(options)?;
        // Discoverable: the login page asks for no email, so the
        // authenticator has to say whose it is.
        if let Some(selection) = options
            .pointer_mut("/publicKey/authenticatorSelection")
            .and_then(serde_json::Value::as_object_mut)
        {
            selection.insert("residentKey".into(), "required".into());
            selection.insert("requireResidentKey".into(), true.into());
        }
        self.registering
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(user_id.to_string(), (state, Instant::now()));
        Ok(options)
    }

    /// The browser's answer to [`Passkeys::start_registration`]: the passkey,
    /// its credential id (base64url) and its JSON, to store.
    pub fn finish_registration(
        &self,
        user_id: &str,
        credential: &RegisterPublicKeyCredential,
    ) -> Result<(String, String)> {
        let (state, started) = self
            .registering
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(user_id)
            .context("no passkey registration is waiting: start again")?;
        if started.elapsed() > CEREMONY {
            bail!("the registration took longer than five minutes: start again");
        }
        let passkey = self
            .webauthn
            .finish_passkey_registration(credential, &state)
            .context("the browser's answer did not register a passkey")?;
        Ok((credential_id(&passkey), serde_json::to_string(&passkey)?))
    }

    /// A login with a passkey, begun: the ceremony's id and the options for
    /// `navigator.credentials.get`. No email: the passkey says whose it is.
    pub fn start_login(&self) -> Result<(String, RequestChallengeResponse)> {
        let mut waiting = self
            .logging_in
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        waiting.retain(|_, (_, started)| started.elapsed() <= CEREMONY);
        if waiting.len() >= WAITING {
            bail!("too many passkey logins are waiting: try again in a few minutes");
        }
        let (options, state) = self
            .webauthn
            .start_discoverable_authentication()
            .context("starting a passkey login")?;
        let ceremony = hex(&crate::users::random::<16>()?);
        waiting.insert(ceremony.clone(), (state, Instant::now()));
        Ok((ceremony, options))
    }

    /// The browser's answer to [`Passkeys::start_login`]. `stored` finds a
    /// passkey by credential id: whose it is, and its JSON. The answer is the
    /// user it proved, and the passkey's JSON with its counter moved on.
    pub fn finish_login(
        &self,
        ceremony: &str,
        credential: &PublicKeyCredential,
        stored: impl FnOnce(&str) -> Result<Option<(String, String)>>,
    ) -> Result<(String, String, String)> {
        let (state, started) = self
            .logging_in
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(ceremony)
            .context("no passkey login is waiting: start again")?;
        if started.elapsed() > CEREMONY {
            bail!("the login took longer than five minutes: start again");
        }
        let (handle_said, raw_id) = self
            .webauthn
            .identify_discoverable_authentication(credential)
            .context("the browser's answer names no passkey")?;
        let id = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(raw_id);
        let (user, body) = stored(&id)?.context("that passkey is not on any account here")?;
        // The handle the authenticator gave is checked against the row,
        // never trusted on its own.
        if handle(&user) != handle_said {
            bail!("that passkey is not on any account here");
        }
        let mut passkey: Passkey = serde_json::from_str(&body)?;
        let result: AuthenticationResult = self
            .webauthn
            .finish_discoverable_authentication_v2(credential, state, &passkey)
            .context("the passkey did not prove it")?;
        passkey.update_credential(&result);
        Ok((user, id, serde_json::to_string(&passkey)?))
    }
}

/// The WebAuthn user handle for a user id: its six bytes, then zeros.
fn handle(user_id: &str) -> Uuid {
    let mut bytes = [0u8; 16];
    for (i, pair) in user_id.as_bytes().chunks(2).take(16).enumerate() {
        bytes[i] = std::str::from_utf8(pair)
            .ok()
            .and_then(|pair| u8::from_str_radix(pair, 16).ok())
            .unwrap_or(0);
    }
    Uuid::from_bytes(bytes)
}

fn credential_id(passkey: &Passkey) -> String {
    let id: &[u8] = passkey.cred_id().as_ref();
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(id)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_handle_is_the_id_padded_and_tells_two_users_apart() {
        let one = handle("0a1b2c3d4e5f");
        assert_eq!(
            one.as_bytes(),
            &[
                0x0a, 0x1b, 0x2c, 0x3d, 0x4e, 0x5f, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0
            ]
        );
        assert_ne!(one, handle("0a1b2c3d4e60"));
    }

    #[test]
    fn the_relying_party_is_the_origin() {
        assert!(Passkeys::new("https://luu.example.com").is_ok());
        assert!(Passkeys::new("not a url").is_err());
    }

    #[test]
    fn a_login_starts_without_an_email_and_a_wrong_ceremony_is_refused() {
        let passkeys = Passkeys::new("https://luu.example.com").unwrap();
        let (ceremony, options) = passkeys.start_login().unwrap();
        assert_eq!(ceremony.len(), 32);
        let options = serde_json::to_value(&options).unwrap();
        assert_eq!(options["publicKey"]["rpId"], "luu.example.com");
        assert!(passkeys.logging_in.lock().unwrap().contains_key(&ceremony));
    }
}
