//! Converts interleaved audio between channel counts and sample rates, for
//! devices that don't take the console's 48 kHz stereo directly.

/// Channel mapping plus linear-interpolation resampling. Keeps state between
/// calls, so a stream can be fed in chunks.
pub struct Converter {
    in_channels: usize,
    out_channels: usize,
    /// Input frames per output frame.
    step: f64,
    /// Position of the next output frame, in input frames, relative to the
    /// start of the current chunk (-1.0 means "the last frame of the
    /// previous chunk").
    pos: f64,
    /// Last input frame of the previous chunk, already channel-mapped.
    prev: Vec<f32>,
    mapped: Vec<f32>,
}

impl Converter {
    pub fn new(in_channels: usize, in_rate: u32, out_channels: usize, out_rate: u32) -> Self {
        let out_channels = out_channels.max(1);
        Self {
            in_channels: in_channels.max(1),
            out_channels,
            step: in_rate as f64 / out_rate as f64,
            pos: 0.0,
            prev: vec![0.0; out_channels],
            mapped: Vec::new(),
        }
    }

    /// Whether this converter changes anything at all.
    pub fn is_identity(&self) -> bool {
        self.in_channels == self.out_channels && self.step == 1.0
    }

    /// Converts `input` and appends the result to `out`.
    pub fn process(&mut self, input: &[f32], out: &mut Vec<f32>) {
        if self.is_identity() {
            out.extend_from_slice(input);
            return;
        }
        let (ic, oc) = (self.in_channels, self.out_channels);
        let frames = input.len() / ic;

        // Channel mapping first: mono is copied to every channel; going down
        // to mono averages; otherwise channels are copied in order (front
        // left/right first) and extra output channels stay silent.
        self.mapped.clear();
        for frame in input.chunks_exact(ic) {
            for c in 0..oc {
                let v = if ic == 1 {
                    frame[0]
                } else if oc == 1 {
                    frame.iter().sum::<f32>() / ic as f32
                } else if c < ic {
                    frame[c]
                } else {
                    0.0
                };
                self.mapped.push(v);
            }
        }
        if self.step == 1.0 {
            out.extend_from_slice(&self.mapped);
            return;
        }

        // Linear interpolation between neighbouring frames; index -1 is the
        // previous chunk's last frame.
        let frame_at = |i: isize, c: usize, mapped: &[f32], prev: &[f32]| -> f32 {
            if i < 0 {
                prev[c]
            } else {
                mapped[i as usize * oc + c]
            }
        };
        // Stop before the last frame: interpolating past it needs the next chunk.
        while self.pos < frames as f64 - 1.0 {
            let i = self.pos.floor() as isize;
            let t = (self.pos - i as f64) as f32;
            for c in 0..oc {
                let a = frame_at(i, c, &self.mapped, &self.prev);
                let b = frame_at(i + 1, c, &self.mapped, &self.prev);
                out.push(a + (b - a) * t);
            }
            self.pos += self.step;
        }
        if frames > 0 {
            self.prev
                .copy_from_slice(&self.mapped[(frames - 1) * oc..frames * oc]);
            self.pos -= frames as f64;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_passes_through() {
        let mut c = Converter::new(2, 48_000, 2, 48_000);
        assert!(c.is_identity());
        let mut out = Vec::new();
        c.process(&[1.0, 2.0, 3.0, 4.0], &mut out);
        assert_eq!(out, [1.0, 2.0, 3.0, 4.0]);
    }

    #[test]
    fn stereo_to_mono_and_surround() {
        let mut out = Vec::new();
        Converter::new(2, 48_000, 1, 48_000).process(&[1.0, 0.0, 0.5, 0.5], &mut out);
        assert_eq!(out, [0.5, 0.5]);
        out.clear();
        Converter::new(2, 48_000, 4, 48_000).process(&[0.1, 0.2], &mut out);
        assert_eq!(out, [0.1, 0.2, 0.0, 0.0]);
        out.clear();
        Converter::new(1, 48_000, 2, 48_000).process(&[0.3], &mut out);
        assert_eq!(out, [0.3, 0.3]);
    }

    #[test]
    fn resampling_keeps_the_rate_across_chunks() {
        // 48 kHz -> 44.1 kHz: 4800 input frames in 10 chunks give ~4410.
        let mut c = Converter::new(2, 48_000, 2, 44_100);
        let mut out = Vec::new();
        let chunk: Vec<f32> = (0..480 * 2).map(|i| (i / 2) as f32 / 480.0).collect();
        for _ in 0..10 {
            c.process(&chunk, &mut out);
        }
        let frames = out.len() / 2;
        assert!((4408..=4411).contains(&frames), "{frames}");
    }

    #[test]
    fn upsampling_interpolates() {
        // Doubling the rate puts midpoints between input frames, including
        // across the chunk boundary.
        let mut c = Converter::new(1, 24_000, 1, 48_000);
        let mut out = Vec::new();
        c.process(&[0.0, 1.0], &mut out);
        c.process(&[2.0, 3.0], &mut out);
        // 3.0 waits for the next chunk.
        assert_eq!(out, [0.0, 0.5, 1.0, 1.5, 2.0, 2.5]);
    }
}
