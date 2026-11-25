//! Tether's API for the Android app, exported to Kotlin with UniFFI.
//!
//! Kotlin creates one [`Tether`] with the app's files directory, uses it to
//! discover, pair and wake consoles, and calls [`Tether::start_stream`] to
//! stream one. Video goes straight from chiaki's network thread to a
//! MediaCodec decoder that draws onto the SurfaceView's surface, which Kotlin
//! hands over once through `NativeSurface.set` (a plain JNI call, see
//! [`surface`]). No per-frame JNI calls.

use std::net::IpAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rp_core::{ConsoleState as RpState, DiscoveryOptions, RegistRequest};
use tether_core::{SecretStore, Store};

mod stream;
pub mod surface;
#[cfg(target_os = "android")]
mod video;

pub use stream::{
    StreamEvent, StreamHandle, StreamListener, StreamOptions, VideoCodec, VideoResolution,
};

uniffi::setup_scaffolding!();

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum TetherError {
    #[error("{message}")]
    Failed { message: String },
}

impl TetherError {
    pub(crate) fn msg(message: impl Into<String>) -> Self {
        Self::Failed {
            message: message.into(),
        }
    }
}

macro_rules! from_error {
    ($($t:ty),*) => {$(
        impl From<$t> for TetherError {
            fn from(e: $t) -> Self {
                Self::msg(e.to_string())
            }
        }
    )*};
}
from_error!(
    rp_core::Error,
    tether_core::StoreError,
    tether_core::AwakeError,
    std::io::Error
);

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum ConsoleState {
    Ready,
    /// Rest mode; can be woken.
    Standby,
    Unknown,
}

impl From<RpState> for ConsoleState {
    fn from(s: RpState) -> Self {
        match s {
            RpState::Ready => Self::Ready,
            RpState::Standby => Self::Standby,
            RpState::Unknown => Self::Unknown,
        }
    }
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct ConsoleInfo {
    /// Stable ID (the console's MAC as 12 hex digits).
    pub host_id: String,
    pub name: String,
    pub address: String,
    pub state: ConsoleState,
    pub paired: bool,
}

/// The app's handle on settings, paired consoles and sessions.
#[derive(uniffi::Object)]
pub struct Tether {
    store: Mutex<Store>,
}

#[uniffi::export]
impl Tether {
    /// Opens (or creates) Tether's data in `data_dir`, normally the app's
    /// `filesDir`. Pairing keys go to `secrets.toml` there, which only the
    /// app can read; moving them into the Android Keystore is milestone 7.
    #[uniffi::constructor]
    pub fn new(data_dir: String) -> Result<Arc<Self>, TetherError> {
        init_logging();
        let dir = std::path::PathBuf::from(data_dir);
        let secrets = SecretStore::File(dir.join("secrets.toml"));
        let store = Store::open(dir, secrets)?;
        Ok(Arc::new(Self {
            store: Mutex::new(store),
        }))
    }

    /// Consoles on the local network, with whether each is paired.
    pub fn discover(&self, timeout_ms: u32) -> Result<Vec<ConsoleInfo>, TetherError> {
        let found = rp_core::discover(
            Duration::from_millis(timeout_ms.into()),
            &DiscoveryOptions::default(),
        )?;
        let store = self.store.lock().unwrap();
        Ok(found
            .into_iter()
            .map(|c| ConsoleInfo {
                paired: store
                    .paired()
                    .iter()
                    .any(|p| p.host_id.eq_ignore_ascii_case(&c.host_id)),
                host_id: c.host_id,
                name: c.name,
                address: c.addr.to_string(),
                state: c.state.into(),
            })
            .collect())
    }

    /// Paired consoles as last seen (state unknown until discovered).
    pub fn paired(&self) -> Vec<ConsoleInfo> {
        self.store
            .lock()
            .unwrap()
            .paired()
            .iter()
            .map(|p| ConsoleInfo {
                host_id: p.host_id.clone(),
                name: p.name.clone(),
                address: p.address.clone(),
                state: ConsoleState::Unknown,
                paired: true,
            })
            .collect()
    }

    /// PSN account ID (base64), if set.
    pub fn account_id(&self) -> Option<String> {
        self.store
            .lock()
            .unwrap()
            .account_id()
            .ok()
            .flatten()
            .map(|id| id.to_base64())
    }

    /// Sets the PSN account ID directly (base64, e.g. from the desktop app's
    /// `config.toml` or chiaki-ng).
    pub fn set_account_id(&self, account_id: String) -> Result<(), TetherError> {
        let id = account_id.trim().parse()?;
        self.store.lock().unwrap().set_account_id(id)?;
        Ok(())
    }

    /// Sony's sign-in page; after signing in the browser lands on a page
    /// whose address goes to [`Tether::finish_login`].
    pub fn login_url(&self) -> String {
        rp_core::psn::login_url()
    }

    /// Trades the address the sign-in page redirected to for the account ID,
    /// stores it and returns it (base64).
    pub fn finish_login(&self, redirect: String) -> Result<String, TetherError> {
        let code = rp_core::psn::parse_redirect(&redirect)?;
        let id = rp_core::psn::fetch_account_id(&code)?;
        self.store.lock().unwrap().set_account_id(id)?;
        Ok(id.to_base64())
    }

    /// Pairs with the console at `address` using the 8-digit code it shows
    /// (Settings > System > Remote Play > Link Device).
    pub fn pair(&self, address: String, pin: String) -> Result<ConsoleInfo, TetherError> {
        let account_id = self
            .store
            .lock()
            .unwrap()
            .account_id()?
            .ok_or_else(|| TetherError::msg("Set your PSN account ID first."))?;
        let pin = rp_core::parse_pin(&pin)?;
        let ip: IpAddr = address
            .trim()
            .parse()
            .map_err(|_| TetherError::msg(format!("{address:?} is not an IP address")))?;
        let console = rp_core::discover(
            Duration::from_secs(3),
            &DiscoveryOptions {
                host: Some(ip),
                ..Default::default()
            },
        )?
        .into_iter()
        .next()
        .ok_or_else(|| TetherError::msg(format!("No console answered at {ip}.")))?;
        if console.state == RpState::Standby {
            return Err(TetherError::msg(format!(
                "{} is in rest mode. Turn it on first.",
                console.name
            )));
        }
        let registered = rp_core::register(&RegistRequest {
            host: ip.to_string(),
            target: console.target,
            account_id,
            pin,
        })?;
        let paired =
            self.store
                .lock()
                .unwrap()
                .add_paired(&registered, &console.name, &ip.to_string())?;
        Ok(ConsoleInfo {
            host_id: paired.host_id,
            name: paired.name,
            address: paired.address,
            state: ConsoleState::Ready,
            paired: true,
        })
    }

    /// Forgets a paired console and its keys.
    pub fn unpair(&self, host_id: String) -> Result<(), TetherError> {
        self.store.lock().unwrap().remove(&host_id)?;
        Ok(())
    }

    /// Wakes the paired console if needed and streams it onto the surface
    /// from `NativeSurface.set`. Events go to `listener` on a background
    /// thread.
    pub fn start_stream(
        &self,
        host_id: String,
        options: StreamOptions,
        listener: Arc<dyn StreamListener>,
    ) -> Result<Arc<StreamHandle>, TetherError> {
        let store = self.store.lock().unwrap();
        let paired = store
            .find(&host_id)
            .cloned()
            .ok_or_else(|| TetherError::msg("That console isn't paired."))?;
        let secrets = store
            .secrets_for(&paired)?
            .ok_or_else(|| TetherError::msg("Keys for that console are missing; pair it again."))?;
        let account_id = store
            .account_id()?
            .ok_or_else(|| TetherError::msg("Set your PSN account ID first."))?;
        drop(store);
        stream::start(paired, secrets, account_id, options, listener)
    }
}

/// Routes `tracing` (and chiaki's log) to logcat under the tag "Tether".
fn init_logging() {
    #[cfg(target_os = "android")]
    {
        use tracing_subscriber::layer::SubscriberExt;
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| {
            if let Ok(layer) = tracing_android::layer("Tether") {
                let subscriber = tracing_subscriber::registry()
                    .with(tracing_subscriber::filter::LevelFilter::INFO)
                    .with(layer);
                let _ = tracing::subscriber::set_global_default(subscriber);
            }
        });
    }
}
