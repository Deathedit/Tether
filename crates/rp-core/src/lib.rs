//! Safe protocol API for PlayStation Remote Play, built on chiaki-ng.
//!
//! All `unsafe` calls into `chiaki-sys` live in this crate.

pub mod discovery;
mod error;
mod ffi;
pub mod psn;
pub mod regist;
pub mod session;
pub mod target;
pub mod wake;

pub use discovery::{
    Console, ConsoleKind, ConsoleState, Discovery, DiscoveryOptions, RunningApp, discover,
};
pub use error::{Error, Result};
pub use psn::AccountId;
pub use regist::{RegistRequest, RegisteredConsole, Registration, parse_pin, register};
pub use session::{
    Codec, Resolution, Session, SessionConfig, SessionEvent, VideoProfile, VideoSample, VideoSink,
};
pub use target::Target;
pub use wake::{parse_regist_key, wake};
