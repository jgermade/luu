//! Who approved, proved.
//!
//! `approve_job` is the authority the whole design is built around — nothing
//! runs behind the gate until it arrives — and until this module it was admitted
//! on the strength of having reached the socket. A bearer token
//! ([`crate::sandbox`]'s sibling in `luu::auth`) answers *who may reach the
//! surface*; it does not answer *who approved*, because every holder can mint
//! approvals indistinguishable from every other holder's and a relay holds it in
//! the clear.
//!
//! So an approval may carry an Ed25519 signature over a canonical rendering of
//! **the grant**, not of the message. A relay that widens `writes` between the
//! person and the gate invalidates what it is relaying.
//!
//! See `RECORD/2026-09-04.signed-approvals.completed.md`.

use std::path::Path;

use ed25519_dalek::{Signature as Ed25519Signature, Signer as _, SigningKey, Verifier as _};
use serde::{Deserialize, Serialize};

use crate::job::{ApprovedBy, JobId};
use crate::sandbox::PolicyError;

/// The first line of every canonical rendering. It is in the signed bytes so a
/// signature made for one shape of approval cannot be replayed against another.
const CANONICAL: &str = "loude-approval v1";

/// How a key is written wherever a person reads or types one.
const PREFIX: &str = "ed25519:";

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ApprovalError {
    /// The canonical form is line-oriented, so a value containing a newline
    /// cannot be encoded. Refused rather than escaped: an escape scheme is a
    /// second parser, and two parsers that disagree about the same bytes are
    /// the class of bug a signature exists to remove.
    #[error("the approval's {field} contains a newline, which cannot be signed")]
    Newline { field: &'static str },
    #[error("this server requires a signed approval and this one carries no signature")]
    Unsigned,
    #[error("no approval key is named `{0}`")]
    UnknownKey(String),
    #[error("the signature does not match what it approves")]
    BadSignature,
    #[error("{0}")]
    Key(String),
}

/// The grant a signature covers: everything the approval widens the job by, in
/// the order the message carries it, plus the session it belongs to.
///
/// The session is here because without it a signature captured on one host
/// replays against another host that numbers its jobs the same way — which two
/// machines on a LAN running the same repository will.
#[derive(Debug, Clone, Copy)]
pub struct Approval<'a> {
    pub session: &'a str,
    pub job: JobId,
    pub files: &'a [String],
    pub writes: &'a [String],
    pub commands: &'a [String],
    pub closes_on: Option<&'a String>,
    pub network: Option<bool>,
    pub egress: Option<&'a Vec<String>>,
}

impl Approval<'_> {
    /// The exact bytes both sides sign and verify.
    ///
    /// Written by this project rather than taken from the wire encoding: a
    /// field reordered by a serde version, or a client that emits `null` where
    /// ours omits the key, would otherwise be a forged approval that verifies
    /// nowhere.
    pub fn canonical(&self) -> Result<String, ApprovalError> {
        let mut out = String::from(CANONICAL);
        out.push('\n');
        line(&mut out, "session", self.session, "session")?;
        out.push_str(&format!("job {}\n", self.job));
        for path in self.files {
            line(&mut out, "files", path, "files")?;
        }
        for path in self.writes {
            line(&mut out, "writes", path, "writes")?;
        }
        for command in self.commands {
            line(&mut out, "commands", command, "commands")?;
        }
        if let Some(closes_on) = self.closes_on {
            line(&mut out, "closes_on", closes_on, "closes_on")?;
        }
        if let Some(network) = self.network {
            out.push_str(&format!("network {network}\n"));
        }
        for domain in self.egress.into_iter().flatten() {
            line(&mut out, "egress", domain, "egress")?;
        }
        Ok(out)
    }
}

fn line(
    out: &mut String,
    key: &str,
    value: &str,
    field: &'static str,
) -> Result<(), ApprovalError> {
    if value.contains('\n') {
        return Err(ApprovalError::Newline { field });
    }
    out.push_str(key);
    out.push(' ');
    out.push_str(value);
    out.push('\n');
    Ok(())
}

/// A signature as it travels on the wire: the name the host's configuration
/// calls the key, and the signature itself in hex.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Signature {
    /// Which configured key signed. Names are per host: the same key may be
    /// called something else on the machine that verifies it.
    pub by: String,
    pub sig: String,
}

/// A private key, and the only thing that can produce a [`Signature`].
pub struct Signer {
    key: SigningKey,
}

impl Signer {
    /// A new key, seeded from `/dev/urandom`.
    ///
    /// Rather than `rand_core` and its feature matrix for one 32-byte read:
    /// this is the whole of what a keypair needs from the system, and every
    /// platform this project runs on has it.
    pub fn generate() -> Result<Self, ApprovalError> {
        use std::io::Read;

        let mut seed = [0u8; 32];
        std::fs::File::open("/dev/urandom")
            .and_then(|mut file| file.read_exact(&mut seed))
            .map_err(|source| ApprovalError::Key(format!("reading /dev/urandom: {source}")))?;
        Ok(Self {
            key: SigningKey::from_bytes(&seed),
        })
    }

    /// Reads `ed25519:<hex>` out of a file. Whitespace around it is ignored so
    /// the file can end with a newline like every other text file.
    pub fn from_file(path: &Path) -> Result<Self, ApprovalError> {
        let text = std::fs::read_to_string(path).map_err(|source| {
            ApprovalError::Key(format!("reading {}: {source}", path.display()))
        })?;
        Self::parse(text.trim())
    }

    pub fn parse(text: &str) -> Result<Self, ApprovalError> {
        let bytes = decode(text)?;
        let seed: [u8; 32] = bytes
            .try_into()
            .map_err(|_| ApprovalError::Key("an ed25519 private key is 32 bytes".into()))?;
        Ok(Self {
            key: SigningKey::from_bytes(&seed),
        })
    }

    /// How the key is written to disk.
    pub fn secret(&self) -> String {
        format!("{PREFIX}{}", hex(self.key.as_bytes()))
    }

    /// How the key is written in `luu.toml`, where it is safe to read.
    pub fn public(&self) -> String {
        format!("{PREFIX}{}", hex(self.key.verifying_key().as_bytes()))
    }

    pub fn sign(
        &self,
        approval: &Approval<'_>,
        by: impl Into<String>,
    ) -> Result<Signature, ApprovalError> {
        let canonical = approval.canonical()?;
        Ok(Signature {
            by: by.into(),
            sig: hex(&self.key.sign(canonical.as_bytes()).to_bytes()),
        })
    }
}

/// One public key, as `luu.toml` names it.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApproverKey {
    pub name: String,
    /// `ed25519:<hex>`, as [`Signer::public`] prints it.
    pub public: String,
}

/// The `[approvals]` block: who may approve, and whether anyone must.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct Approvers {
    /// Whether an *unsigned* approval is admitted. Off by default: a loopback
    /// session with a person at the keyboard is the run every measurement in
    /// this repository was made under, and it has no key to sign with.
    ///
    /// It never decides whether a *wrong* signature is admitted. That one is
    /// always refused.
    #[serde(default)]
    pub required: bool,
    #[serde(default, rename = "key")]
    pub keys: Vec<ApproverKey>,
}

/// The file `luu.toml` is, read for its `[approvals]` block — a third reader of
/// one file, for the same reason `[worker]` is a second one.
#[derive(Debug, Clone, Default, Deserialize)]
struct ApprovalsFile {
    #[serde(default)]
    approvals: Option<Approvers>,
}

impl Approvers {
    /// Reads `[approvals]`. A file without one is not an error — it is a
    /// `luu.toml` from before this existed, and its approvals are the
    /// operator's own.
    pub fn from_file(path: &Path) -> Result<Self, PolicyError> {
        let text = std::fs::read_to_string(path).map_err(|source| PolicyError::Read {
            path: path.display().to_string(),
            source,
        })?;
        Self::from_toml(&text).map_err(|source| PolicyError::Parse {
            path: path.display().to_string(),
            source,
        })
    }

    pub fn from_toml(text: &str) -> Result<Self, toml::de::Error> {
        let file: ApprovalsFile = toml::from_str(text)?;
        Ok(file.approvals.unwrap_or_default())
    }

    /// Whether this approval may proceed, and who it will be recorded as.
    ///
    /// A signature is verified whenever one is present, so a wrong one is
    /// refused even where signatures are optional. `required` only decides
    /// what an absent signature means.
    pub fn admits(
        &self,
        approval: &Approval<'_>,
        signature: Option<&Signature>,
    ) -> Result<ApprovedBy, ApprovalError> {
        let Some(signature) = signature else {
            return match self.required {
                true => Err(ApprovalError::Unsigned),
                false => Ok(ApprovedBy::Operator),
            };
        };

        let key = self
            .keys
            .iter()
            .find(|key| key.name == signature.by)
            .ok_or_else(|| ApprovalError::UnknownKey(signature.by.clone()))?;
        let verifying = verifying_key(&key.public)?;
        let bytes: [u8; 64] = decode(&signature.sig)?
            .try_into()
            .map_err(|_| ApprovalError::BadSignature)?;
        verifying
            .verify(
                approval.canonical()?.as_bytes(),
                &Ed25519Signature::from_bytes(&bytes),
            )
            .map_err(|_| ApprovalError::BadSignature)?;
        Ok(ApprovedBy::Key {
            name: key.name.clone(),
        })
    }
}

fn verifying_key(text: &str) -> Result<ed25519_dalek::VerifyingKey, ApprovalError> {
    let bytes: [u8; 32] = decode(text)?
        .try_into()
        .map_err(|_| ApprovalError::Key("an ed25519 public key is 32 bytes".into()))?;
    ed25519_dalek::VerifyingKey::from_bytes(&bytes).map_err(|source| {
        ApprovalError::Key(format!("this is not an ed25519 public key: {source}"))
    })
}

/// `ed25519:<hex>` in, bytes out. The prefix is optional on the way in so a
/// bare signature — which is not a key and has no prefix — reads with the same
/// function.
fn decode(text: &str) -> Result<Vec<u8>, ApprovalError> {
    let text = text.trim();
    let digits = text.strip_prefix(PREFIX).unwrap_or(text);
    if digits.is_empty() || !digits.len().is_multiple_of(2) {
        return Err(ApprovalError::Key(format!("{text}: not hex")));
    }
    (0..digits.len())
        .step_by(2)
        .map(|at| {
            u8::from_str_radix(&digits[at..at + 2], 16)
                .map_err(|_| ApprovalError::Key(format!("{text}: not hex")))
        })
        .collect()
}

/// Hex rather than base64, for the same reason `/dev/urandom` is read directly:
/// it is small enough that a dependency costs more than the code.
fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grant<'a>(session: &'a str, writes: &'a [String]) -> Approval<'a> {
        Approval {
            session,
            job: 3,
            files: &[],
            writes,
            commands: &[],
            closes_on: None,
            network: None,
            egress: None,
        }
    }

    #[test]
    fn the_canonical_form_is_the_grant_and_nothing_else() {
        let writes = vec!["src/main.rs".to_string()];
        let approval = grant("s1", &writes);
        assert_eq!(
            approval.canonical().unwrap(),
            "loude-approval v1\nsession s1\njob 3\nwrites src/main.rs\n"
        );
    }

    #[test]
    fn a_value_with_a_newline_is_refused_rather_than_escaped() {
        let writes = vec!["src/main.rs\nsecrets".to_string()];
        assert_eq!(
            grant("s1", &writes).canonical(),
            Err(ApprovalError::Newline { field: "writes" })
        );
    }

    #[test]
    fn a_signature_verifies_against_the_key_that_made_it() {
        let signer = Signer::generate().unwrap();
        let approvers = Approvers {
            required: true,
            keys: vec![ApproverKey {
                name: "jgermade".into(),
                public: signer.public(),
            }],
        };
        let writes = vec!["src/main.rs".to_string()];
        let approval = grant("s1", &writes);
        let signature = signer.sign(&approval, "jgermade").unwrap();

        assert_eq!(
            approvers.admits(&approval, Some(&signature)),
            Ok(ApprovedBy::Key {
                name: "jgermade".into()
            })
        );
    }

    #[test]
    fn a_grant_widened_after_signing_does_not_verify() {
        let signer = Signer::generate().unwrap();
        let approvers = Approvers {
            required: true,
            keys: vec![ApproverKey {
                name: "jgermade".into(),
                public: signer.public(),
            }],
        };
        let signed = vec!["src/main.rs".to_string()];
        let signature = signer.sign(&grant("s1", &signed), "jgermade").unwrap();

        // The relay's version of the same approval: one more tree.
        let widened = vec!["src/main.rs".to_string(), "/".to_string()];
        assert_eq!(
            approvers.admits(&grant("s1", &widened), Some(&signature)),
            Err(ApprovalError::BadSignature)
        );
    }

    #[test]
    fn a_signature_does_not_replay_against_another_session() {
        let signer = Signer::generate().unwrap();
        let approvers = Approvers {
            required: true,
            keys: vec![ApproverKey {
                name: "jgermade".into(),
                public: signer.public(),
            }],
        };
        let writes = vec!["src/main.rs".to_string()];
        let signature = signer.sign(&grant("s1", &writes), "jgermade").unwrap();

        assert_eq!(
            approvers.admits(&grant("s2", &writes), Some(&signature)),
            Err(ApprovalError::BadSignature)
        );
    }

    #[test]
    fn a_name_no_key_answers_to_is_refused() {
        let signer = Signer::generate().unwrap();
        let approvers = Approvers {
            required: true,
            keys: vec![ApproverKey {
                name: "someone-else".into(),
                public: signer.public(),
            }],
        };
        let writes = vec!["src/main.rs".to_string()];
        let approval = grant("s1", &writes);
        let signature = signer.sign(&approval, "jgermade").unwrap();

        assert_eq!(
            approvers.admits(&approval, Some(&signature)),
            Err(ApprovalError::UnknownKey("jgermade".into()))
        );
    }

    #[test]
    fn an_unsigned_approval_is_the_operators_where_none_is_required() {
        let writes = vec!["src/main.rs".to_string()];
        assert_eq!(
            Approvers::default().admits(&grant("s1", &writes), None),
            Ok(ApprovedBy::Operator)
        );
        assert_eq!(
            Approvers {
                required: true,
                keys: Vec::new(),
            }
            .admits(&grant("s1", &writes), None),
            Err(ApprovalError::Unsigned)
        );
    }

    #[test]
    fn a_key_survives_the_round_trip_through_a_file_line() {
        let signer = Signer::generate().unwrap();
        let read_back = Signer::parse(&signer.secret()).unwrap();
        assert_eq!(read_back.public(), signer.public());
    }

    #[test]
    fn a_file_without_the_block_is_not_an_error() {
        let approvers = Approvers::from_toml("[sandbox]\nnetwork = false\n").unwrap();
        assert!(!approvers.required);
        assert!(approvers.keys.is_empty());
    }

    #[test]
    fn the_block_reads_as_written() {
        let approvers = Approvers::from_toml(
            "[approvals]\nrequired = true\n\n[[approvals.key]]\nname = \"jgermade\"\npublic = \"ed25519:00\"\n",
        )
        .unwrap();
        assert!(approvers.required);
        assert_eq!(approvers.keys[0].name, "jgermade");
    }
}
