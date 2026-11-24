//! Tether's shared app logic, used by the desktop app and (through UniFFI)
//! the Android app.
//!
//! For now: settings and paired consoles ([`Store`]) and waking a console
//! before connecting ([`ensure_awake`]). Audio and stats come later.

pub mod awake;
pub mod config;
pub mod secrets;
mod store;

pub use awake::{AwakeError, WakeProgress, ensure_awake};
pub use config::{Config, PairedConsole};
pub use secrets::{ConsoleSecrets, SecretStore};
pub use store::{Store, StoreError};
