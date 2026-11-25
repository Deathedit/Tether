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
    ButtonBits, ControllerInput, StreamEvent, StreamHandle, StreamListener, StreamOptions,
    StreamStats, VideoCodec, VideoResolution, button_bits,
};

uniffi::setup_scaffolding!();

/// Errors reach Kotlin as `TetherException.Failed`, whose `message` is the
/// text below (a "flat" error: no fields that could clash with Throwable's).
#[derive(Debug, thiserror::Error, uniffi::Error)]
#[uniffi(flat_error)]
pub enum TetherError {
    #[error("{0}")]
    Failed(String),
}

impl TetherError {
    pub(crate) fn msg(message: impl Into<String>) -> Self {
        Self::Failed(message.into())
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
    /// `filesDir`. Pairing keys go to `storage` (Keystore-encrypted on
    /// Android); keys an older version kept in `secrets.toml` move there.
    /// Without `storage` they stay in `secrets.toml`.
    #[uniffi::constructor]
    pub fn new(
        data_dir: String,
        storage: Option<Arc<dyn SecureStorage>>,
    ) -> Result<Arc<Self>, TetherError> {
        init_logging();
        let dir = std::path::PathBuf::from(data_dir);
        let file = dir.join("secrets.toml");
        let secrets = match storage {
            Some(storage) => {
                let store = SecretStore::External(Arc::new(StorageBackend(storage)));
                match store.import_file(&file) {
                    Ok(0) => {}
                    Ok(n) => tracing::info!("moved {n} console key(s) into secure storage"),
                    Err(e) => tracing::warn!("could not move keys into secure storage: {e}"),
                }
                store
            }
            None => SecretStore::File(file),
        };
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

    /// Puts a paired console into rest mode. chiaki can only do that from a
    /// session, so this starts a small one, sends "go to bed" and ends it.
    /// Does nothing if the console is already resting.
    pub fn rest_mode(&self, host_id: String) -> Result<(), TetherError> {
        let (paired, secrets, account_id) = self.session_parts(&host_id)?;
        let ip: IpAddr = paired
            .address
            .parse()
            .map_err(|_| TetherError::msg("bad address"))?;
        let console = rp_core::discover(
            Duration::from_secs(2),
            &DiscoveryOptions {
                host: Some(ip),
                ..Default::default()
            },
        )?
        .into_iter()
        .next()
        .ok_or_else(|| TetherError::msg(format!("{} isn't answering.", paired.name)))?;
        if console.state == RpState::Standby {
            return Ok(());
        }
        let config = rp_core::SessionConfig {
            host: ip.to_string(),
            target: paired.target(),
            regist_key: secrets.regist_key,
            rp_key: secrets.rp_key,
            account_id,
            video: rp_core::VideoProfile::preset(
                rp_core::Resolution::P360,
                30,
                rp_core::Codec::H264,
            ),
        };
        let (session, events) = rp_core::Session::start(&config, Box::new(|_| true))?;
        let deadline = std::time::Instant::now() + Duration::from_secs(15);
        loop {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            match events.recv_timeout(left) {
                Ok(rp_core::SessionEvent::Connected) => {
                    session.goto_bed()?;
                    // The console ends the session itself; give it a moment.
                    let _ = events.recv_timeout(Duration::from_secs(3));
                    return Ok(());
                }
                Ok(rp_core::SessionEvent::LoginPinRequest { .. }) => {
                    return Err(TetherError::msg(
                        "The console asks for its passcode; use Rest mode from a stream.",
                    ));
                }
                Ok(rp_core::SessionEvent::Quit { reason, .. }) => {
                    return Err(TetherError::msg(reason));
                }
                Ok(_) => {}
                Err(_) => return Err(TetherError::msg("The console didn't respond in time.")),
            }
        }
    }

    /// One network round trip to the console at `address`, in milliseconds,
    /// or `None` if it didn't answer within half a second. For the home
    /// screen's latency graph (before a stream gives better numbers).
    pub fn probe_latency(&self, address: String) -> Option<f32> {
        probe_latency(&address)
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
        let (paired, secrets, account_id) = self.session_parts(&host_id)?;
        stream::start(paired, secrets, account_id, options, listener)
    }
}

impl Tether {
    /// A paired console with its keys and the account ID, or why not.
    fn session_parts(
        &self,
        host_id: &str,
    ) -> Result<
        (
            tether_core::PairedConsole,
            tether_core::ConsoleSecrets,
            rp_core::AccountId,
        ),
        TetherError,
    > {
        let store = self.store.lock().unwrap();
        let paired = store
            .find(host_id)
            .cloned()
            .ok_or_else(|| TetherError::msg("That console isn't paired."))?;
        let secrets = store
            .secrets_for(&paired)?
            .ok_or_else(|| TetherError::msg("Keys for that console are missing; pair it again."))?;
        let account_id = store
            .account_id()?
            .ok_or_else(|| TetherError::msg("Set your PSN account ID first."))?;
        Ok((paired, secrets, account_id))
    }
}

/// Times one discovery request/reply to the console's discovery port.
fn probe_latency(address: &str) -> Option<f32> {
    use std::net::{SocketAddr, UdpSocket};
    let ip: IpAddr = address.trim().parse().ok()?;
    let target = SocketAddr::new(ip, 9302);
    let socket = UdpSocket::bind(if ip.is_ipv4() { "0.0.0.0:0" } else { "[::]:0" }).ok()?;
    socket
        .set_read_timeout(Some(Duration::from_millis(500)))
        .ok()?;
    let started = std::time::Instant::now();
    socket
        .send_to(
            b"SRCH * HTTP/1.1\ndevice-discovery-protocol-version:00030010\n",
            target,
        )
        .ok()?;
    let mut buf = [0u8; 512];
    loop {
        let (_, from) = socket.recv_from(&mut buf).ok()?;
        if from.ip() == ip {
            return Some(started.elapsed().as_secs_f32() * 1000.0);
        }
    }
}

/// Secret storage the app provides (Android: values encrypted with a
/// Keystore key). Keys look like `console:<HOSTID>`.
#[uniffi::export(with_foreign)]
pub trait SecureStorage: Send + Sync {
    fn get(&self, key: String) -> Option<String>;
    /// Returns false if the value could not be stored.
    fn put(&self, key: String, value: String) -> bool;
    fn remove(&self, key: String);
}

struct StorageBackend(Arc<dyn SecureStorage>);

impl tether_core::SecretBackend for StorageBackend {
    fn get(&self, key: &str) -> Result<Option<String>, String> {
        Ok(self.0.get(key.to_owned()))
    }
    fn set(&self, key: &str, value: &str) -> Result<(), String> {
        if self.0.put(key.to_owned(), value.to_owned()) {
            Ok(())
        } else {
            Err("secure storage refused the value".into())
        }
    }
    fn delete(&self, key: &str) -> Result<(), String> {
        self.0.remove(key.to_owned());
        Ok(())
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
            // Panic messages go to stderr, which Android discards; log them.
            let default_hook = std::panic::take_hook();
            std::panic::set_hook(Box::new(move |info| {
                tracing::error!("panic: {info}");
                default_hook(info);
            }));
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn latency_probe_against_fake_console() {
        let console = std::net::UdpSocket::bind("127.0.0.1:9302").expect("port 9302 free");
        let responder = std::thread::spawn(move || {
            let mut buf = [0u8; 512];
            let (n, from) = console.recv_from(&mut buf).unwrap();
            assert!(buf[..n].starts_with(b"SRCH"));
            console.send_to(b"HTTP/1.1 200 Ok\n", from).unwrap();
        });
        let ms = probe_latency("127.0.0.1").expect("fake console answers");
        responder.join().unwrap();
        assert!((0.0..500.0).contains(&ms));
        assert_eq!(probe_latency("not an ip"), None);
    }
}
