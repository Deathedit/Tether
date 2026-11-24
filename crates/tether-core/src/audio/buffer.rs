//! The jitter buffer between the network thread (which delivers audio in
//! bursts) and the output device (which pulls it at a steady rate).

use std::collections::VecDeque;

/// Interleaved samples waiting to be played.
///
/// Playback starts once `target` frames are buffered. If the device drains
/// it (an underrun) it outputs silence and waits for `target` frames again,
/// so one late packet costs one short gap instead of a stream of crackles.
/// If more than `max` frames pile up (the console's clock running faster
/// than the device's, or a burst after a stall) the oldest are dropped back
/// down to `target`, which keeps latency bounded.
pub struct JitterBuffer {
    samples: VecDeque<f32>,
    channels: usize,
    target: usize,
    max: usize,
    playing: bool,
    stats: BufferStats,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct BufferStats {
    /// Times the device ran dry mid-playback.
    pub underruns: u64,
    /// Frames dropped to keep latency down.
    pub dropped_frames: u64,
}

impl JitterBuffer {
    /// `target` and `max` are in frames (samples per channel).
    pub fn new(channels: usize, target: usize, max: usize) -> Self {
        let channels = channels.max(1);
        let max = max.max(target);
        Self {
            samples: VecDeque::with_capacity((max + target) * channels),
            channels,
            target,
            max,
            playing: false,
            stats: BufferStats::default(),
        }
    }

    pub fn channels(&self) -> usize {
        self.channels
    }

    /// Frames currently buffered.
    pub fn frames(&self) -> usize {
        self.samples.len() / self.channels
    }

    pub fn stats(&self) -> BufferStats {
        self.stats
    }

    /// Adds interleaved samples (a whole number of frames).
    pub fn push(&mut self, samples: &[f32]) {
        self.samples.extend(samples);
        if self.frames() > self.max {
            let drop = self.frames() - self.target;
            self.samples.drain(..drop * self.channels);
            self.stats.dropped_frames += drop as u64;
        }
    }

    /// Fills `out` (interleaved, same channel count) with the next samples,
    /// or silence while buffering.
    pub fn pop(&mut self, out: &mut [f32]) {
        if !self.playing {
            if self.frames() >= self.target && !self.samples.is_empty() {
                self.playing = true;
            } else {
                out.fill(0.0);
                return;
            }
        }
        let n = out.len().min(self.samples.len());
        for (dst, src) in out.iter_mut().zip(self.samples.drain(..n)) {
            *dst = src;
        }
        if n < out.len() {
            out[n..].fill(0.0);
            self.playing = false;
            self.stats.underruns += 1;
        }
    }

    /// Forgets everything (e.g. when the stream format changes).
    pub fn clear(&mut self) {
        self.samples.clear();
        self.playing = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ramp(frames: usize, start: f32) -> Vec<f32> {
        (0..frames * 2).map(|i| start + (i / 2) as f32).collect()
    }

    #[test]
    fn waits_for_target_then_plays_in_order() {
        let mut b = JitterBuffer::new(2, 4, 16);
        let mut out = [1.0f32; 4];
        b.push(&ramp(2, 0.0));
        b.pop(&mut out);
        assert_eq!(out, [0.0; 4], "silence until 4 frames are buffered");
        assert_eq!(b.frames(), 2);
        b.push(&ramp(2, 2.0));
        b.pop(&mut out);
        assert_eq!(out, [0.0, 0.0, 1.0, 1.0]);
        b.pop(&mut out);
        assert_eq!(out, [2.0, 2.0, 3.0, 3.0]);
        assert_eq!(b.stats().underruns, 0);
    }

    #[test]
    fn underrun_pads_with_silence_and_rebuffers() {
        let mut b = JitterBuffer::new(2, 2, 16);
        b.push(&ramp(3, 0.0));
        let mut out = [9.0f32; 8];
        b.pop(&mut out);
        assert_eq!(out, [0.0, 0.0, 1.0, 1.0, 2.0, 2.0, 0.0, 0.0]);
        assert_eq!(b.stats().underruns, 1);
        // Buffering again: one frame isn't enough.
        b.push(&ramp(1, 5.0));
        let mut out = [9.0f32; 2];
        b.pop(&mut out);
        assert_eq!(out, [0.0, 0.0]);
        b.push(&ramp(1, 6.0));
        b.pop(&mut out);
        assert_eq!(out, [5.0, 5.0]);
    }

    #[test]
    fn overflow_drops_oldest_down_to_target() {
        let mut b = JitterBuffer::new(2, 4, 8);
        b.push(&ramp(8, 0.0));
        assert_eq!(b.frames(), 8);
        b.push(&ramp(1, 8.0));
        assert_eq!(b.frames(), 4);
        assert_eq!(b.stats().dropped_frames, 5);
        let mut out = [0.0f32; 2];
        b.pop(&mut out);
        assert_eq!(out, [5.0, 5.0], "newest audio kept");
    }
}
