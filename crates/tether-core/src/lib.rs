//! Tether's shared app logic, used by the desktop app and (through UniFFI)
//! the Android app.
//!
//! For now: settings and paired consoles ([`Store`]). Sessions, audio and
//! stats come in later milestones.

pub mod config;
pub mod secrets;
mod store;

pub use config::{Config, PairedConsole};
pub use secrets::{ConsoleSecrets, SecretStore};
pub use store::{Store, StoreError};
