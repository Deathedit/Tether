//! Turns DualSense haptics into plain rumble.
//!
//! When the console streams to a DualSense-capable client, most PS5 games
//! send vibration as a haptics *audio* stream (16-bit stereo PCM at 3 kHz,
//! one track per actuator) instead of rumble events. Playing that through
//! the DualSense's own audio interface is the real thing (TODO); this is the
//! fallback chiaki-ng uses for any controller: take the loudness of the
//! haptics signal and use it as rumble strength.

/// Packets averaged into one rumble update (3 × 10 ms).
const PACKETS_PER_RUMBLE: u32 = 3;
/// Quieter packets count as silence.
const MIN_STRENGTH: u32 = 100;
/// Some controllers drop the low 9 bits of the rumble value; weaker
/// non-zero values are raised to this so they still register.
const MIN_RUMBLE: u16 = 1 << 9;

#[derive(Default)]
pub struct HapticsRumble {
    sum: u32,
    packets: u32,
    on: bool,
}

impl HapticsRumble {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feeds one haptics packet (interleaved left/right samples). Every few
    /// packets returns the rumble to apply, as (low, high) motor strength on
    /// SDL's 0..=65535 scale; `Some((0, 0))` stops a running rumble.
    pub fn push(&mut self, samples: &[i16]) -> Option<(u16, u16)> {
        self.sum += packet_strength(samples) as u32;
        self.packets += 1;
        if self.packets < PACKETS_PER_RUMBLE {
            return None;
        }
        let strength = (self.sum / PACKETS_PER_RUMBLE) as u16;
        self.sum = 0;
        self.packets = 0;
        if strength == 0 && !self.on {
            return None;
        }
        self.on = strength > 0;
        Some((strength, strength))
    }
}

/// Mean absolute amplitude of the louder side, doubled, or 0 if too quiet.
fn packet_strength(samples: &[i16]) -> u16 {
    let frames = samples.len() / 2;
    if frames == 0 {
        return 0;
    }
    let (mut left, mut right) = (0u64, 0u64);
    for f in samples.chunks_exact(2) {
        left += f[0].unsigned_abs() as u64 * 2;
        right += f[1].unsigned_abs() as u64 * 2;
    }
    let level = (left.max(right) / frames as u64) as u32;
    if level <= MIN_STRENGTH {
        return 0;
    }
    (level.min(u16::MAX as u32) as u16).max(MIN_RUMBLE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn silence_never_rumbles() {
        let mut h = HapticsRumble::new();
        for _ in 0..10 {
            assert_eq!(h.push(&[0; 60]), None);
        }
    }

    #[test]
    fn signal_rumbles_then_stops_once() {
        let mut h = HapticsRumble::new();
        let loud: Vec<i16> = (0..60)
            .map(|i| if i % 2 == 0 { 8000 } else { -8000 })
            .collect();
        assert_eq!(h.push(&loud), None);
        assert_eq!(h.push(&loud), None);
        let (low, high) = h.push(&loud).unwrap();
        assert_eq!((low, high), (16000, 16000));
        // Back to silence: one stop, then nothing.
        h.push(&[0; 60]);
        h.push(&[0; 60]);
        assert_eq!(h.push(&[0; 60]), Some((0, 0)));
        for _ in 0..3 {
            assert_eq!(h.push(&[0; 60]), None);
        }
    }

    #[test]
    fn weak_signal_is_raised_to_the_minimum() {
        assert_eq!(packet_strength(&[0, 120, 0, -120]), MIN_RUMBLE);
        assert_eq!(packet_strength(&[10, 10]), 0, "below the noise floor");
    }
}
