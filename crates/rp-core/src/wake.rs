//! Waking a console from rest mode.

use std::ffi::CString;

use chiaki_sys as sys;

use crate::error::{Error, Result, check};
use crate::ffi;

/// Parses the registration key that wake packets carry.
///
/// Registration (pairing) gives a key of up to 8 hex digits, stored as raw
/// bytes padded with NULs. The console expects it as a hex number.
pub fn parse_regist_key(key: &str) -> Result<u64> {
    let key = key.trim().trim_end_matches('\0');
    if key.is_empty() || key.len() > 8 {
        return Err(Error::InvalidRegistKey);
    }
    u64::from_str_radix(key, 16).map_err(|_| Error::InvalidRegistKey)
}

/// Sends a wake-up packet to the console at `host` (IP address or host name).
///
/// `regist_key` comes from registering with the console (pairing). `ps5`
/// picks the discovery port the packet goes to.
pub fn wake(host: &str, regist_key: &str, ps5: bool) -> Result<()> {
    let credential = parse_regist_key(regist_key)?;
    let c_host = CString::new(host).map_err(|_| Error::InvalidHost(host.to_owned()))?;
    let log = ffi::log();
    // SAFETY: valid log and C string; a null discovery makes chiaki create a
    // temporary socket for this one packet.
    check(unsafe {
        sys::chiaki_discovery_wakeup(log, std::ptr::null_mut(), c_host.as_ptr(), credential, ps5)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regist_key_parsing() {
        assert_eq!(parse_regist_key("1a2b3c4d").unwrap(), 0x1a2b_3c4d);
        assert_eq!(parse_regist_key("ff\0\0\0\0").unwrap(), 0xff);
        assert_eq!(parse_regist_key(" 0 ").unwrap(), 0);
        assert!(parse_regist_key("").is_err());
        assert!(parse_regist_key("123456789").is_err());
        assert!(parse_regist_key("xyz").is_err());
    }
}
