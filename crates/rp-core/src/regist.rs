//! Registering (pairing) with a console using the 8-digit code it shows.
//!
//! On the console: Settings › System › Remote Play › Pair Device. The console
//! shows a code for a few minutes; registration must happen while that screen
//! is open.

use std::ffi::{CString, c_void};

use chiaki_sys as sys;
use crossbeam_channel::{Receiver, Sender};

use crate::error::{Error, Result, check};
use crate::ffi::CapturingLog;
use crate::psn::AccountId;
use crate::target::Target;

/// What to register with.
#[derive(Debug, Clone)]
pub struct RegistRequest {
    /// IP address or host name of the console.
    pub host: String,
    /// From discovery (`Console::target`).
    pub target: Target,
    pub account_id: AccountId,
    /// The 8-digit code shown on the console.
    pub pin: u32,
}

/// Credentials the console hands out on successful registration. Keep the
/// keys secret: together they let anyone on the network start a session.
#[derive(Clone, PartialEq, Eq)]
pub struct RegisteredConsole {
    pub target: Target,
    pub server_mac: [u8; 6],
    pub nickname: String,
    /// Raw `rp_regist_key`: a short hex string padded with NULs to 16 bytes.
    pub regist_key: [u8; 16],
    pub rp_key_type: u32,
    pub rp_key: [u8; 16],
}

impl RegisteredConsole {
    /// The registration key as text, as `wake` expects it.
    pub fn regist_key_hex(&self) -> String {
        let end = self.regist_key.iter().position(|&b| b == 0).unwrap_or(16);
        String::from_utf8_lossy(&self.regist_key[..end]).into_owned()
    }

    /// The console's MAC as a 12-digit upper-case hex string. This matches
    /// `Console::host_id` from discovery.
    pub fn host_id(&self) -> String {
        self.server_mac.iter().map(|b| format!("{b:02X}")).collect()
    }

    /// # Safety
    /// `host` must be a valid registered host from chiaki.
    unsafe fn from_raw(host: &sys::ChiakiRegisteredHost) -> Self {
        let nickname_bytes: Vec<u8> = host
            .server_nickname
            .iter()
            .map(|&c| c as u8)
            .take_while(|&b| b != 0)
            .collect();
        let mut regist_key = [0u8; 16];
        for (dst, &src) in regist_key.iter_mut().zip(host.rp_regist_key.iter()) {
            *dst = src as u8;
        }
        RegisteredConsole {
            target: Target::from_sys(host.target),
            server_mac: host.server_mac,
            nickname: String::from_utf8_lossy(&nickname_bytes).into_owned(),
            regist_key,
            rp_key_type: host.rp_key_type,
            rp_key: host.rp_key,
        }
    }
}

impl std::fmt::Debug for RegisteredConsole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RegisteredConsole")
            .field("target", &self.target)
            .field("host_id", &self.host_id())
            .field("nickname", &self.nickname)
            .field("keys", &"<redacted>")
            .finish()
    }
}

/// Parses the 8-digit code shown on the console. Spaces are ignored.
pub fn parse_pin(pin: &str) -> Result<u32> {
    let digits: String = pin.chars().filter(|c| !c.is_whitespace()).collect();
    if digits.len() != 8 || !digits.chars().all(|c| c.is_ascii_digit()) {
        return Err(Error::InvalidPin);
    }
    Ok(digits.parse().expect("8 ASCII digits fit in u32"))
}

enum Outcome {
    Success(RegisteredConsole),
    Failed,
    Canceled,
}

/// A registration in progress. Cancels and cleans up when dropped.
pub struct Registration {
    regist: Box<sys::ChiakiRegist>,
    rx: Receiver<Outcome>,
    // Both must outlive chiaki's registration thread; see Drop.
    _tx: Box<Sender<Outcome>>,
    log: Box<CapturingLog>,
}

// SAFETY: chiaki's registration runs on its own thread; this handle only
// calls stop and fini.
unsafe impl Send for Registration {}

impl Registration {
    pub fn start(request: &RegistRequest) -> Result<Self> {
        if request.target == Target::PS4_8 || request.target == Target::PS4_UNKNOWN {
            return Err(Error::Unsupported(
                "registering with PS4 firmware older than 7.0",
            ));
        }
        let host = CString::new(request.host.as_str())
            .map_err(|_| Error::InvalidHost(request.host.clone()))?;
        let log = CapturingLog::new();
        let (tx, rx) = crossbeam_channel::bounded(1);
        let tx = Box::new(tx);

        let info = sys::ChiakiRegistInfo {
            target: request.target.to_sys(),
            host: host.as_ptr(),
            broadcast: false,
            psn_online_id: std::ptr::null(),
            psn_account_id: *request.account_id.as_bytes(),
            pin: request.pin,
            console_pin: 0,
            holepunch_info: std::ptr::null_mut(),
            rudp: std::ptr::null_mut(),
        };

        let mut regist = Box::new(sys::ChiakiRegist::default());
        // SAFETY: `regist` is boxed (stable address for chiaki's thread);
        // chiaki copies `host`. `tx` and `log` live until `fini` in Drop.
        check(unsafe {
            sys::chiaki_regist_start(
                &mut *regist,
                log.as_ptr(),
                &info,
                Some(regist_cb),
                &*tx as *const Sender<Outcome> as *mut c_void,
            )
        })?;

        Ok(Self {
            regist,
            rx,
            _tx: tx,
            log,
        })
    }

    /// Asks the registration to stop. `wait` then returns `Error::Canceled`.
    pub fn cancel(&self) {
        // SAFETY: started in `start`; stop only signals chiaki's stop pipe.
        unsafe { sys::chiaki_regist_stop(&*self.regist as *const _ as *mut sys::ChiakiRegist) };
    }

    /// Blocks until the console accepts or rejects the registration. chiaki
    /// gives up on its own after a few seconds without an answer.
    pub fn wait(self) -> Result<RegisteredConsole> {
        match self.rx.recv() {
            Ok(Outcome::Success(console)) => Ok(console),
            Ok(Outcome::Canceled) => Err(Error::Canceled),
            Ok(Outcome::Failed) | Err(_) => Err(Error::RegistrationFailed {
                reason: self.failure_reason(),
            }),
        }
    }

    fn failure_reason(&self) -> String {
        let errors = self.log.errors();
        // The console's own reason, when it gave one, is the most useful line.
        errors
            .iter()
            .rev()
            .find(|e| e.contains("Reason"))
            .or(errors.last())
            .cloned()
            .unwrap_or_else(|| "no details from the console".to_owned())
    }
}

impl Drop for Registration {
    fn drop(&mut self) {
        // SAFETY: started in `start`. Stopping a finished registration is a
        // no-op; fini joins the thread, after which no callback can run.
        unsafe {
            sys::chiaki_regist_stop(&mut *self.regist);
            sys::chiaki_regist_fini(&mut *self.regist);
        }
    }
}

unsafe extern "C" fn regist_cb(event: *mut sys::ChiakiRegistEvent, user: *mut c_void) {
    if event.is_null() || user.is_null() {
        return;
    }
    // SAFETY: `user` is the boxed Sender from `start`; `event` is valid for
    // this call.
    let (tx, event) = unsafe { (&*(user as *const Sender<Outcome>), &*event) };
    let outcome = match event.type_ {
        sys::CHIAKI_REGIST_EVENT_TYPE_FINISHED_SUCCESS if !event.registered_host.is_null() => {
            // SAFETY: chiaki passes a valid host with a success event.
            Outcome::Success(unsafe { RegisteredConsole::from_raw(&*event.registered_host) })
        }
        sys::CHIAKI_REGIST_EVENT_TYPE_FINISHED_CANCELED => Outcome::Canceled,
        _ => Outcome::Failed,
    };
    let _ = tx.try_send(outcome);
}

/// Registers with a console and waits for the result.
pub fn register(request: &RegistRequest) -> Result<RegisteredConsole> {
    Registration::start(request)?.wait()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pin_parsing() {
        assert_eq!(parse_pin("12345678").unwrap(), 12_345_678);
        assert_eq!(parse_pin("1234 5678").unwrap(), 12_345_678);
        assert_eq!(parse_pin("00001234").unwrap(), 1234);
        assert!(parse_pin("1234567").is_err());
        assert!(parse_pin("123456789").is_err());
        assert!(parse_pin("1234567a").is_err());
    }

    #[test]
    fn regist_key_and_host_id() {
        let mut regist_key = [0u8; 16];
        regist_key[..8].copy_from_slice(b"1a2b3c4d");
        let console = RegisteredConsole {
            target: Target::PS5_1,
            server_mac: [0x90, 0x47, 0x48, 0x52, 0x21, 0xdd],
            nickname: "PS5-309".into(),
            regist_key,
            rp_key_type: 0,
            rp_key: [7; 16],
        };
        assert_eq!(console.regist_key_hex(), "1a2b3c4d");
        assert_eq!(console.host_id(), "9047485221DD");
        assert!(!format!("{console:?}").contains("1a2b3c4d"));
    }
}
