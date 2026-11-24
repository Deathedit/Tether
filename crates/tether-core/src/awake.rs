//! Making sure a paired console is on before connecting.

use std::time::{Duration, Instant};

use rp_core::{Console, ConsoleState, DiscoveryOptions};

use crate::config::PairedConsole;
use crate::secrets::ConsoleSecrets;

/// Resend the wake packet this often while waiting. A packet sent while the
/// console is still going into rest mode is ignored, so one is not enough.
const RESEND_EVERY: Duration = Duration::from_secs(5);
const PROBE_TIMEOUT: Duration = Duration::from_millis(1500);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WakeProgress {
    /// A wake packet was sent; the console is in rest mode or not answering.
    Waking,
    /// Still waiting for the console to report ready.
    Waiting { state: Option<ConsoleState> },
}

#[derive(Debug, thiserror::Error)]
pub enum AwakeError {
    #[error("{name} did not come out of rest mode within {secs} s")]
    Timeout { name: String, secs: u64 },
    #[error(transparent)]
    Protocol(#[from] rp_core::Error),
}

/// Returns once the console reports ready, waking it if needed.
pub fn ensure_awake(
    console: &PairedConsole,
    secrets: &ConsoleSecrets,
    timeout: Duration,
    mut progress: impl FnMut(WakeProgress),
) -> Result<Console, AwakeError> {
    let host = console
        .address
        .parse()
        .map_err(|_| rp_core::Error::InvalidHost(console.address.clone()))?;
    let options = DiscoveryOptions {
        host: Some(host),
        ..Default::default()
    };
    let deadline = Instant::now() + timeout;
    let mut last_wake: Option<Instant> = None;

    loop {
        let found = rp_core::discover(PROBE_TIMEOUT, &options)?
            .into_iter()
            .next();
        let state = found.as_ref().map(|c| c.state);
        if let Some(c) = found.filter(|c| c.state == ConsoleState::Ready) {
            return Ok(c);
        }
        if Instant::now() >= deadline {
            return Err(AwakeError::Timeout {
                name: console.name.clone(),
                secs: timeout.as_secs(),
            });
        }
        if last_wake.is_none_or(|t| t.elapsed() >= RESEND_EVERY) {
            rp_core::wake(
                &console.address,
                &secrets.regist_key_hex(),
                console.target().is_ps5(),
            )?;
            last_wake = Some(Instant::now());
            progress(WakeProgress::Waking);
        } else {
            progress(WakeProgress::Waiting { state });
        }
    }
}
