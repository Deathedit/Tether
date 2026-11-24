//! `config.toml`: settings and the list of paired consoles. Holds nothing
//! secret; keys live in the [`SecretStore`](crate::SecretStore).

use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::settings::{LastSession, StreamSettings};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Config {
    /// PSN account ID (base64), from `tether login`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub psn_account_id: Option<String>,

    #[serde(default)]
    pub stream: StreamSettings,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_session: Option<LastSession>,

    #[serde(default, rename = "console", skip_serializing_if = "Vec::is_empty")]
    pub consoles: Vec<PairedConsole>,
}

/// A console Tether has registered with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairedConsole {
    /// The console's MAC as 12 hex digits; same as discovery's host ID.
    pub host_id: String,
    pub name: String,
    /// Last address the console was reached at.
    pub address: String,
    /// chiaki `ChiakiTarget` value (see `rp_core::Target::raw`).
    pub target: u32,
}

impl PairedConsole {
    pub fn target(&self) -> rp_core::Target {
        rp_core::Target::from_raw(self.target).unwrap_or(rp_core::Target::PS5_1)
    }

    /// Whether `query` names this console: its host ID, name or address,
    /// ignoring case.
    pub fn matches(&self, query: &str) -> bool {
        let q = query.trim();
        self.host_id.eq_ignore_ascii_case(q)
            || self.name.eq_ignore_ascii_case(q)
            || self.address == q
    }
}

impl Config {
    /// Reads the file, or returns an empty config if it doesn't exist yet.
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        match std::fs::read_to_string(path) {
            Ok(text) => Ok(toml::from_str(&text)?),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e.into()),
        }
    }

    /// Writes the file atomically (temp file + rename).
    pub fn save(&self, path: &Path) -> Result<(), ConfigError> {
        let text = toml::to_string_pretty(self)?;
        write_atomic(path, text.as_bytes())?;
        Ok(())
    }

    pub fn find(&self, query: &str) -> Option<&PairedConsole> {
        self.consoles.iter().find(|c| c.matches(query))
    }

    /// Adds a console, replacing any entry with the same host ID.
    pub fn upsert(&mut self, console: PairedConsole) {
        match self
            .consoles
            .iter_mut()
            .find(|c| c.host_id.eq_ignore_ascii_case(&console.host_id))
        {
            Some(existing) => *existing = console,
            None => self.consoles.push(console),
        }
    }
}

pub(crate) fn write_atomic(path: &Path, contents: &[u8]) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, contents)?;
    std::fs::rename(&tmp, path)
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error("config file is not valid TOML: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("could not write config: {0}")]
    Serialize(#[from] toml::ser::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn console(id: &str, name: &str) -> PairedConsole {
        PairedConsole {
            host_id: id.into(),
            name: name.into(),
            address: "192.168.1.222".into(),
            target: 1_000_100,
        }
    }

    #[test]
    fn round_trip_and_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join("config.toml");
        assert_eq!(Config::load(&path).unwrap(), Config::default());

        let mut config = Config {
            psn_account_id: Some("AQAAAAAAAAA=".into()),
            consoles: vec![],
            last_session: Some(LastSession {
                console: "PS5-309".into(),
                started_at: 1_700_000_000,
                duration_secs: 3600,
                video: "1080p60".into(),
                frames_lost: 2,
            }),
            ..Default::default()
        };
        config.upsert(console("9047485221DD", "PS5-309"));
        config.save(&path).unwrap();
        assert_eq!(Config::load(&path).unwrap(), config);

        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("[[console]]"), "{text}");
    }

    #[test]
    fn find_and_upsert() {
        let mut config = Config::default();
        config.upsert(console("9047485221DD", "PS5-309"));
        config.upsert(console("9047485221dd", "Living Room"));
        assert_eq!(config.consoles.len(), 1);
        assert_eq!(config.find("living room").unwrap().name, "Living Room");
        assert!(config.find("9047485221DD").is_some());
        assert!(config.find("192.168.1.222").is_some());
        assert!(config.find("nope").is_none());
    }
}
