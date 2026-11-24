use std::ffi::CStr;

use chiaki_sys as sys;

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// chiaki-ng returned an error code.
    #[error("{message} (chiaki error {code})")]
    Chiaki { code: i64, message: String },

    #[error("invalid registration key: expected 1 to 8 hex digits")]
    InvalidRegistKey,

    #[error("invalid host name: {0}")]
    InvalidHost(String),

    #[error(transparent)]
    Io(#[from] std::io::Error),
}

impl Error {
    pub(crate) fn from_code(code: sys::ChiakiErrorCode) -> Self {
        // SAFETY: chiaki_error_string returns a pointer to a static string.
        let message = unsafe { CStr::from_ptr(sys::chiaki_error_string(code)) }
            .to_string_lossy()
            .into_owned();
        Error::Chiaki {
            code: code as i64,
            message,
        }
    }
}

/// Turns a chiaki error code into a `Result`.
pub(crate) fn check(code: sys::ChiakiErrorCode) -> Result<()> {
    if code == sys::CHIAKI_ERR_SUCCESS {
        Ok(())
    } else {
        Err(Error::from_code(code))
    }
}
