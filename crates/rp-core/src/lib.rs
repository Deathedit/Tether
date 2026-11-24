//! Safe protocol API for PlayStation Remote Play, built on chiaki-ng.
//!
//! All `unsafe` calls into `chiaki-sys` live in this crate.

pub mod discovery;
mod error;
mod ffi;
pub mod wake;

pub use discovery::{
    Console, ConsoleKind, ConsoleState, Discovery, DiscoveryOptions, RunningApp, discover,
};
pub use error::{Error, Result};
pub use wake::{parse_regist_key, wake};
