//! Console generation and firmware family, as chiaki-ng's `ChiakiTarget`.

use std::fmt;

use chiaki_sys as sys;

/// Which protocol variant to speak with a console.
///
/// Wraps chiaki's `ChiakiTarget`, whose numeric values are stable, so
/// [`Target::raw`] is safe to store in config files.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Target(u32);

#[allow(clippy::unnecessary_cast)] // C enums are i32 on MSVC, u32 elsewhere
impl Target {
    pub const PS4_UNKNOWN: Target = Target(sys::CHIAKI_TARGET_PS4_UNKNOWN as u32);
    pub const PS4_8: Target = Target(sys::CHIAKI_TARGET_PS4_8 as u32);
    pub const PS4_9: Target = Target(sys::CHIAKI_TARGET_PS4_9 as u32);
    pub const PS4_10: Target = Target(sys::CHIAKI_TARGET_PS4_10 as u32);
    pub const PS5_UNKNOWN: Target = Target(sys::CHIAKI_TARGET_PS5_UNKNOWN as u32);
    pub const PS5_1: Target = Target(sys::CHIAKI_TARGET_PS5_1 as u32);

    const ALL: [Target; 6] = [
        Self::PS4_UNKNOWN,
        Self::PS4_8,
        Self::PS4_9,
        Self::PS4_10,
        Self::PS5_UNKNOWN,
        Self::PS5_1,
    ];

    /// The stable numeric value, for storing.
    pub fn raw(self) -> u32 {
        self.0
    }

    /// Parses a stored value. Returns `None` for values chiaki doesn't define.
    pub fn from_raw(raw: u32) -> Option<Self> {
        Self::ALL.into_iter().find(|t| t.0 == raw)
    }

    pub fn is_ps5(self) -> bool {
        self.0 >= Self::PS5_UNKNOWN.0
    }

    pub(crate) fn to_sys(self) -> sys::ChiakiTarget {
        self.0 as sys::ChiakiTarget
    }

    pub(crate) fn from_sys(target: sys::ChiakiTarget) -> Self {
        Target(target as u32)
    }
}

impl fmt::Display for Target {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match *self {
            Self::PS5_1 => "PS5",
            Self::PS5_UNKNOWN => "PS5 (pre-release firmware)",
            Self::PS4_10 => "PS4 8.0+",
            Self::PS4_9 => "PS4 7.x",
            Self::PS4_8 => "PS4 < 7.0",
            _ => "unknown",
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_round_trip() {
        for t in Target::ALL {
            assert_eq!(Target::from_raw(t.raw()), Some(t));
        }
        assert_eq!(Target::PS5_1.raw(), 1_000_100);
        assert_eq!(Target::from_raw(42), None);
        assert!(Target::PS5_1.is_ps5());
        assert!(!Target::PS4_10.is_ps5());
    }
}
