//! Tether's shared app logic, used by the desktop app and (through UniFFI)
//! the Android app.
//!
//! Settings and paired consoles ([`Store`]), stream settings
//! ([`StreamSettings`]), waking a console before connecting
//! ([`ensure_awake`]), session audio ([`AudioPlayer`], [`Microphone`]) and
//! haptics-to-rumble ([`HapticsRumble`]). Stats come later.

pub mod audio;
pub mod awake;
pub mod config;
pub mod haptics;
pub mod secrets;
pub mod settings;
mod store;

pub use audio::{AudioError, AudioOptions, AudioPlayer, Microphone};
pub use awake::{AwakeError, WakeProgress, ensure_awake};
pub use config::{Config, PairedConsole};
pub use haptics::HapticsRumble;
pub use secrets::{ConsoleSecrets, SecretStore};
pub use settings::{DecoderPreference, LastSession, StreamSettings, VideoCodec, VideoResolution};
pub use store::{Store, StoreError};
