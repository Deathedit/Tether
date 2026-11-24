//! Shared glue for calling chiaki-ng: one-time library init, a log that
//! forwards to `tracing`, and conversions for C strings and socket addresses.

use std::ffi::{CStr, c_char, c_void};
use std::net::SocketAddr;
use std::sync::OnceLock;

use chiaki_sys as sys;

/// A `ChiakiLog` that lives for the whole program.
struct GlobalLog(Box<sys::ChiakiLog>);

// SAFETY: the log is written once in `init` and only read afterwards. chiaki's
// threads only read `level_mask`, `cb` and `user`.
unsafe impl Send for GlobalLog {}
unsafe impl Sync for GlobalLog {}

static LOG: OnceLock<GlobalLog> = OnceLock::new();

/// Initialises chiaki-ng (sockets on Windows) and returns the shared log.
/// Safe to call many times; the work happens once.
pub(crate) fn log() -> *mut sys::ChiakiLog {
    let log = LOG.get_or_init(|| {
        // SAFETY: plain init call with no arguments.
        let err = unsafe { sys::chiaki_lib_init() };
        if err != sys::CHIAKI_ERR_SUCCESS {
            tracing::error!(target: "chiaki", "chiaki_lib_init failed: {}", crate::Error::from_code(err));
        }

        let mut log = Box::new(sys::ChiakiLog::default());
        // SAFETY: `log` is a valid, boxed (stable address) ChiakiLog.
        unsafe {
            sys::chiaki_log_init(&mut *log, level_mask(), Some(log_cb), std::ptr::null_mut())
        };
        GlobalLog(log)
    });
    &*log.0 as *const sys::ChiakiLog as *mut sys::ChiakiLog
}

/// Levels to ask chiaki for. VERBOSE is very chatty (per-packet), so it is
/// only formatted when someone is listening at trace level.
fn level_mask() -> u32 {
    let mut mask: u32 = sys::CHIAKI_LOG_ALL;
    if !tracing::enabled!(target: "chiaki", tracing::Level::TRACE) {
        // C enums are i32 on MSVC and u32 elsewhere, so the cast is needed on
        // some targets only.
        #[allow(clippy::unnecessary_cast)]
        let verbose = sys::CHIAKI_LOG_VERBOSE as u32;
        mask &= !verbose;
    }
    mask
}

unsafe extern "C" fn log_cb(level: sys::ChiakiLogLevel, msg: *const c_char, _user: *mut c_void) {
    if msg.is_null() {
        return;
    }
    // SAFETY: chiaki passes a null-terminated string valid for this call.
    let msg = unsafe { CStr::from_ptr(msg) }.to_string_lossy();
    forward(level, &msg);
}

fn forward(level: sys::ChiakiLogLevel, msg: &str) {
    match level {
        sys::CHIAKI_LOG_ERROR => tracing::error!(target: "chiaki", "{msg}"),
        sys::CHIAKI_LOG_WARNING => tracing::warn!(target: "chiaki", "{msg}"),
        sys::CHIAKI_LOG_INFO => tracing::info!(target: "chiaki", "{msg}"),
        sys::CHIAKI_LOG_DEBUG => tracing::debug!(target: "chiaki", "{msg}"),
        _ => tracing::trace!(target: "chiaki", "{msg}"),
    }
}

/// A log for one operation (e.g. a registration) that forwards to `tracing`
/// like the global one and also keeps chiaki's error messages, so a failure
/// can report why.
pub(crate) struct CapturingLog {
    log: sys::ChiakiLog,
    errors: std::sync::Mutex<Vec<String>>,
}

impl CapturingLog {
    pub(crate) fn new() -> Box<Self> {
        log(); // make sure chiaki_lib_init ran
        let mut this = Box::new(CapturingLog {
            log: sys::ChiakiLog::default(),
            errors: std::sync::Mutex::new(Vec::new()),
        });
        let user = &*this as *const CapturingLog as *mut c_void;
        // SAFETY: `this` is boxed, so `user` stays valid as long as the box.
        unsafe { sys::chiaki_log_init(&mut this.log, level_mask(), Some(capturing_cb), user) };
        this
    }

    /// Pointer to hand to chiaki. Valid while `self` is alive.
    pub(crate) fn as_ptr(&self) -> *mut sys::ChiakiLog {
        &self.log as *const sys::ChiakiLog as *mut sys::ChiakiLog
    }

    pub(crate) fn errors(&self) -> Vec<String> {
        self.errors.lock().unwrap().clone()
    }
}

unsafe extern "C" fn capturing_cb(
    level: sys::ChiakiLogLevel,
    msg: *const c_char,
    user: *mut c_void,
) {
    if msg.is_null() || user.is_null() {
        return;
    }
    // SAFETY: chiaki passes a valid string; `user` is the CapturingLog that
    // owns this ChiakiLog.
    let (msg, this) = unsafe {
        (
            CStr::from_ptr(msg).to_string_lossy(),
            &*(user as *const CapturingLog),
        )
    };
    forward(level, &msg);
    if level == sys::CHIAKI_LOG_ERROR {
        this.errors.lock().unwrap().push(msg.into_owned());
    }
}

// SAFETY: chiaki only reads the ChiakiLog fields; the error list is a Mutex.
unsafe impl Send for CapturingLog {}
unsafe impl Sync for CapturingLog {}

/// Copies a nullable C string into an owned `String`.
///
/// # Safety
/// `ptr` must be null or point to a null-terminated string.
pub(crate) unsafe fn opt_string(ptr: *const c_char) -> Option<String> {
    if ptr.is_null() {
        None
    } else {
        // SAFETY: guaranteed by the caller.
        Some(
            unsafe { CStr::from_ptr(ptr) }
                .to_string_lossy()
                .into_owned(),
        )
    }
}

/// Converts a Rust socket address into chiaki's `sockaddr_storage` plus the
/// length of the address inside it.
pub(crate) fn to_sockaddr_storage(addr: SocketAddr) -> (sys::sockaddr_storage, usize) {
    let src = socket2::SockAddr::from(addr);
    let len = src.len() as usize;
    let mut storage = sys::sockaddr_storage::default();
    assert!(len <= std::mem::size_of::<sys::sockaddr_storage>());
    // SAFETY: both buffers are at least `len` bytes and don't overlap.
    unsafe {
        std::ptr::copy_nonoverlapping(
            src.as_ptr() as *const u8,
            &mut storage as *mut _ as *mut u8,
            len,
        );
    }
    (storage, len)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, SocketAddrV4};

    #[test]
    fn sockaddr_round_trip() {
        let addr = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(192, 168, 1, 20), 9302));
        let (storage, len) = to_sockaddr_storage(addr);
        // SAFETY: storage holds a valid address of length `len`.
        let back = unsafe {
            socket2::SockAddr::try_init(|ptr, plen| {
                std::ptr::copy_nonoverlapping(
                    &storage as *const _ as *const u8,
                    ptr as *mut u8,
                    len,
                );
                *plen = len as _;
                Ok(())
            })
        }
        .unwrap()
        .1;
        assert_eq!(back.as_socket(), Some(addr));
    }
}
