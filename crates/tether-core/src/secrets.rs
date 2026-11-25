//! Where the per-console keys from registration are kept.
//!
//! Desktop: the OS credential store (Windows Credential Manager, or the
//! Secret Service on Linux, i.e. gnome-keyring or KWallet). If none is
//! available, which is common on minimal NixOS or Arch setups without a
//! keyring daemon, a `secrets.toml` readable only by the user is used instead.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

use rp_core::RegisteredConsole;

use crate::config::write_atomic;

/// Credential-store service name (desktop only; Android uses the file store).
#[cfg(any(windows, target_os = "linux", target_os = "macos"))]
const SERVICE: &str = "tether";

/// The keys a console gave us when we registered.
#[derive(Clone, PartialEq, Eq)]
pub struct ConsoleSecrets {
    pub regist_key: [u8; 16],
    pub rp_key: [u8; 16],
    pub rp_key_type: u32,
}

impl ConsoleSecrets {
    pub fn from_registered(console: &RegisteredConsole) -> Self {
        Self {
            regist_key: console.regist_key,
            rp_key: console.rp_key,
            rp_key_type: console.rp_key_type,
        }
    }

    /// Registration key as text (what wake packets carry).
    pub fn regist_key_hex(&self) -> String {
        let end = self.regist_key.iter().position(|&b| b == 0).unwrap_or(16);
        String::from_utf8_lossy(&self.regist_key[..end]).into_owned()
    }

    fn encode(&self) -> String {
        format!(
            "v1:{}:{}:{}",
            hex::encode(self.regist_key),
            hex::encode(self.rp_key),
            self.rp_key_type
        )
    }

    fn decode(s: &str) -> Option<Self> {
        let mut parts = s.trim().split(':');
        if parts.next()? != "v1" {
            return None;
        }
        let regist_key = hex::decode(parts.next()?).ok()?.try_into().ok()?;
        let rp_key = hex::decode(parts.next()?).ok()?.try_into().ok()?;
        let rp_key_type = parts.next()?.parse().ok()?;
        parts.next().is_none().then_some(Self {
            regist_key,
            rp_key,
            rp_key_type,
        })
    }
}

impl fmt::Debug for ConsoleSecrets {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ConsoleSecrets(<redacted>)")
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SecretsError {
    #[error("credential store: {0}")]
    Keyring(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("stored keys for {0} are unreadable")]
    Corrupt(String),
}

#[derive(Debug, Clone)]
pub enum SecretStore {
    /// The OS credential store.
    Keyring,
    /// Fallback file, mode 0600 on Unix.
    File(PathBuf),
}

impl SecretStore {
    /// Uses the OS credential store if it works, else `<dir>/secrets.toml`.
    pub fn open_default(dir: &Path) -> Self {
        let file = Self::File(dir.join("secrets.toml"));
        #[cfg(any(windows, target_os = "linux", target_os = "macos"))]
        match keyring_probe() {
            Ok(()) => return Self::Keyring,
            Err(e) => {
                tracing::warn!(
                    "OS credential store unavailable ({e}); keeping keys in {}",
                    dir.join("secrets.toml").display()
                );
            }
        }
        file
    }

    /// Human-readable description, for status output.
    pub fn describe(&self) -> String {
        match self {
            Self::Keyring => "system credential store".to_owned(),
            Self::File(path) => path.display().to_string(),
        }
    }

    pub fn set(&self, host_id: &str, secrets: &ConsoleSecrets) -> Result<(), SecretsError> {
        match self {
            #[cfg(any(windows, target_os = "linux", target_os = "macos"))]
            Self::Keyring => entry(host_id)?
                .set_password(&secrets.encode())
                .map_err(|e| SecretsError::Keyring(e.to_string())),
            #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
            Self::Keyring => Err(SecretsError::Keyring(
                "not available on this platform".into(),
            )),
            Self::File(path) => {
                let mut map = read_file(path)?;
                map.insert(key(host_id), secrets.encode());
                write_file(path, &map)
            }
        }
    }

    pub fn get(&self, host_id: &str) -> Result<Option<ConsoleSecrets>, SecretsError> {
        let encoded = match self {
            #[cfg(any(windows, target_os = "linux", target_os = "macos"))]
            Self::Keyring => match entry(host_id)?.get_password() {
                Ok(s) => Some(s),
                Err(keyring::Error::NoEntry) => None,
                Err(e) => return Err(SecretsError::Keyring(e.to_string())),
            },
            #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
            Self::Keyring => None,
            Self::File(path) => read_file(path)?.remove(&key(host_id)),
        };
        encoded
            .map(|s| {
                ConsoleSecrets::decode(&s).ok_or_else(|| SecretsError::Corrupt(host_id.to_owned()))
            })
            .transpose()
    }

    pub fn delete(&self, host_id: &str) -> Result<(), SecretsError> {
        match self {
            #[cfg(any(windows, target_os = "linux", target_os = "macos"))]
            Self::Keyring => match entry(host_id)?.delete_credential() {
                Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
                Err(e) => Err(SecretsError::Keyring(e.to_string())),
            },
            #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
            Self::Keyring => Ok(()),
            Self::File(path) => {
                let mut map = read_file(path)?;
                if map.remove(&key(host_id)).is_some() {
                    write_file(path, &map)?;
                }
                Ok(())
            }
        }
    }
}

fn key(host_id: &str) -> String {
    format!("console:{}", host_id.to_ascii_uppercase())
}

#[cfg(any(windows, target_os = "linux", target_os = "macos"))]
fn entry(host_id: &str) -> Result<keyring::Entry, SecretsError> {
    keyring::Entry::new(SERVICE, &key(host_id)).map_err(|e| SecretsError::Keyring(e.to_string()))
}

/// A read of an entry that doesn't exist: succeeds with `NoEntry` when the
/// store works, fails when e.g. no Secret Service daemon is running.
#[cfg(any(windows, target_os = "linux", target_os = "macos"))]
fn keyring_probe() -> Result<(), String> {
    let entry = keyring::Entry::new(SERVICE, "probe").map_err(|e| e.to_string())?;
    match entry.get_password() {
        Ok(_) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}

fn read_file(path: &Path) -> Result<BTreeMap<String, String>, SecretsError> {
    match std::fs::read_to_string(path) {
        Ok(text) => {
            toml::from_str(&text).map_err(|_| SecretsError::Corrupt(path.display().to_string()))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
        Err(e) => Err(e.into()),
    }
}

fn write_file(path: &Path, map: &BTreeMap<String, String>) -> Result<(), SecretsError> {
    let text = toml::to_string(map).expect("string map always serializes");
    write_atomic(path, text.as_bytes())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> ConsoleSecrets {
        let mut regist_key = [0u8; 16];
        regist_key[..8].copy_from_slice(b"1a2b3c4d");
        ConsoleSecrets {
            regist_key,
            rp_key: [0xab; 16],
            rp_key_type: 2,
        }
    }

    #[test]
    fn encode_round_trip() {
        let s = sample();
        assert_eq!(ConsoleSecrets::decode(&s.encode()), Some(s.clone()));
        assert_eq!(s.regist_key_hex(), "1a2b3c4d");
        assert!(ConsoleSecrets::decode("v2:00").is_none());
        assert!(!format!("{s:?}").contains("1a2b"));
    }

    #[test]
    fn file_store() {
        let dir = tempfile::tempdir().unwrap();
        let store = SecretStore::File(dir.path().join("secrets.toml"));
        assert_eq!(store.get("9047485221DD").unwrap(), None);
        store.set("9047485221DD", &sample()).unwrap();
        assert_eq!(store.get("9047485221dd").unwrap(), Some(sample()));
        store.delete("9047485221DD").unwrap();
        assert_eq!(store.get("9047485221DD").unwrap(), None);
    }
}
