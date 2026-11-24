//! [`Store`]: config file + secret store, as one handle.

use std::path::{Path, PathBuf};

use rp_core::{AccountId, RegisteredConsole};

use crate::config::{Config, ConfigError, PairedConsole};
use crate::secrets::{ConsoleSecrets, SecretStore, SecretsError};

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error(transparent)]
    Config(#[from] ConfigError),
    #[error(transparent)]
    Secrets(#[from] SecretsError),
    #[error("could not find a config directory for this user")]
    NoConfigDir,
    #[error("stored PSN account ID is invalid; run `tether login` again")]
    BadAccountId,
}

pub struct Store {
    dir: PathBuf,
    config: Config,
    secrets: SecretStore,
}

impl Store {
    /// The per-user config dir: `~/.config/tether` on Linux,
    /// `%APPDATA%\tether\config` on Windows.
    pub fn default_dir() -> Result<PathBuf, StoreError> {
        directories::ProjectDirs::from("", "", "tether")
            .map(|d| d.config_dir().to_path_buf())
            .ok_or(StoreError::NoConfigDir)
    }

    /// Opens the default store with the best available secret store.
    pub fn open_default() -> Result<Self, StoreError> {
        let dir = Self::default_dir()?;
        let secrets = SecretStore::open_default(&dir);
        Self::open(dir, secrets)
    }

    pub fn open(dir: PathBuf, secrets: SecretStore) -> Result<Self, StoreError> {
        let config = Config::load(&dir.join("config.toml"))?;
        Ok(Self {
            dir,
            config,
            secrets,
        })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn config_path(&self) -> PathBuf {
        self.dir.join("config.toml")
    }

    pub fn secret_store(&self) -> &SecretStore {
        &self.secrets
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    fn save(&self) -> Result<(), StoreError> {
        Ok(self.config.save(&self.config_path())?)
    }

    pub fn account_id(&self) -> Result<Option<AccountId>, StoreError> {
        self.config
            .psn_account_id
            .as_deref()
            .map(|s| s.parse().map_err(|_| StoreError::BadAccountId))
            .transpose()
    }

    pub fn set_account_id(&mut self, id: AccountId) -> Result<(), StoreError> {
        self.config.psn_account_id = Some(id.to_base64());
        self.save()
    }

    pub fn paired(&self) -> &[PairedConsole] {
        &self.config.consoles
    }

    /// Finds a paired console by host ID, name or address.
    pub fn find(&self, query: &str) -> Option<&PairedConsole> {
        self.config.find(query)
    }

    /// Records a successful registration: keys to the secret store, the rest
    /// to the config file. Keys are written first, so a failure never leaves
    /// a console listed without its keys.
    pub fn add_paired(
        &mut self,
        console: &RegisteredConsole,
        name: &str,
        address: &str,
    ) -> Result<PairedConsole, StoreError> {
        let host_id = console.host_id();
        self.secrets
            .set(&host_id, &ConsoleSecrets::from_registered(console))?;
        let paired = PairedConsole {
            host_id,
            name: name.to_owned(),
            address: address.to_owned(),
            target: console.target.raw(),
        };
        self.config.upsert(paired.clone());
        self.save()?;
        Ok(paired)
    }

    pub fn secrets_for(
        &self,
        console: &PairedConsole,
    ) -> Result<Option<ConsoleSecrets>, StoreError> {
        Ok(self.secrets.get(&console.host_id)?)
    }

    /// Remembers a console's new address (e.g. after DHCP gave it another).
    pub fn update_address(&mut self, host_id: &str, address: &str) -> Result<(), StoreError> {
        let Some(c) = self
            .config
            .consoles
            .iter_mut()
            .find(|c| c.host_id.eq_ignore_ascii_case(host_id))
        else {
            return Ok(());
        };
        if c.address != address {
            c.address = address.to_owned();
            self.save()?;
        }
        Ok(())
    }

    /// Forgets a console and its keys. Returns it, or `None` if not paired.
    pub fn remove(&mut self, query: &str) -> Result<Option<PairedConsole>, StoreError> {
        let Some(index) = self.config.consoles.iter().position(|c| c.matches(query)) else {
            return Ok(None);
        };
        let console = self.config.consoles.remove(index);
        self.save()?;
        self.secrets.delete(&console.host_id)?;
        Ok(Some(console))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rp_core::Target;

    fn registered() -> RegisteredConsole {
        let mut regist_key = [0u8; 16];
        regist_key[..8].copy_from_slice(b"1a2b3c4d");
        RegisteredConsole {
            target: Target::PS5_1,
            server_mac: [0x90, 0x47, 0x48, 0x52, 0x21, 0xdd],
            nickname: "PS5-309".into(),
            regist_key,
            rp_key_type: 0,
            rp_key: [9; 16],
        }
    }

    #[test]
    fn pair_find_remove() {
        let dir = tempfile::tempdir().unwrap();
        let secrets = SecretStore::File(dir.path().join("secrets.toml"));
        let mut store = Store::open(dir.path().to_path_buf(), secrets.clone()).unwrap();

        store.set_account_id(AccountId::from_user_id(1)).unwrap();
        let paired = store
            .add_paired(&registered(), "PS5-309", "192.168.1.222")
            .unwrap();
        assert_eq!(paired.host_id, "9047485221DD");

        // Reopen from disk.
        let mut store = Store::open(dir.path().to_path_buf(), secrets).unwrap();
        assert_eq!(
            store.account_id().unwrap(),
            Some(AccountId::from_user_id(1))
        );
        let found = store.find("ps5-309").unwrap().clone();
        assert_eq!(found.target(), Target::PS5_1);
        assert_eq!(
            store.secrets_for(&found).unwrap().unwrap().regist_key_hex(),
            "1a2b3c4d"
        );

        store
            .update_address("9047485221dd", "192.168.1.50")
            .unwrap();
        assert!(store.find("192.168.1.50").is_some());

        assert!(store.remove("PS5-309").unwrap().is_some());
        assert!(store.paired().is_empty());
        assert_eq!(store.secrets_for(&found).unwrap(), None);
        assert!(store.remove("PS5-309").unwrap().is_none());
    }
}
