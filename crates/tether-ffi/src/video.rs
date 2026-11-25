//! Hardware video decoding on Android: MediaCodec, driven from Rust through
//! the NDK, rendering straight onto the SurfaceView's ANativeWindow. No
//! frame copies through Java and no per-frame JNI calls.

use std::thread::JoinHandle;
use std::time::Duration;

use crossbeam_channel::Receiver;
use ndk::media::media_codec::{
    DequeuedInputBufferResult, DequeuedOutputBufferInfoResult, MediaCodec, MediaCodecDirection,
};
use ndk::media::media_format::MediaFormat;
use ndk::native_window::NativeWindow;
use rp_core::VideoProfile;

use crate::stream::StreamEvent;

pub struct EncodedFrame {
    pub data: Vec<u8>,
    pub frames_lost: i32,
}

#[derive(Default)]
pub struct DecodeStats {
    /// Frames handed to the decoder.
    pub queued: u64,
    /// Decoded frames drawn on the surface.
    pub shown: u64,
    /// Decoded but not drawn because newer frames were already waiting.
    pub dropped: u64,
    pub errors: u64,
    /// The codec MediaCodec picked, e.g. "c2.qti.avc.decoder".
    pub decoder: String,
}

/// How long to wait for a free input buffer before giving up on a frame.
const INPUT_WAIT: Duration = Duration::from_millis(10);
const INPUT_TRIES: u32 = 5;
/// After queueing a frame, wait this long (a few times) for it to come out,
/// so it's drawn now instead of when the next frame arrives.
const OUTPUT_WAIT: Duration = Duration::from_millis(4);
const OUTPUT_TRIES: u32 = 4;

/// Starts the decoder thread. Returns once the codec is configured, or the
/// reason it couldn't be. The thread ends when `frames` closes.
pub fn spawn_decoder(
    profile: VideoProfile,
    window: NativeWindow,
    frames: Receiver<EncodedFrame>,
    on_event: impl Fn(StreamEvent) + Send + 'static,
    on_error: impl Fn() + Send + 'static,
) -> Result<JoinHandle<DecodeStats>, String> {
    let (ready_tx, ready_rx) = crossbeam_channel::bounded::<Result<(), String>>(1);
    let thread = std::thread::Builder::new()
        .name("video-decode".into())
        .spawn(move || {
            let codec = match open_codec(&profile, &window) {
                Ok(c) => {
                    let _ = ready_tx.send(Ok(()));
                    c
                }
                Err(e) => {
                    let _ = ready_tx.send(Err(e));
                    return DecodeStats::default();
                }
            };
            let stats = decode_loop(&codec, &profile, &frames, &on_event, &on_error);
            let _ = codec.stop();
            stats
        })
        .map_err(|e| e.to_string())?;
    ready_rx
        .recv()
        .map_err(|_| "decoder thread died".to_string())??;
    Ok(thread)
}

fn open_codec(profile: &VideoProfile, window: &NativeWindow) -> Result<MediaCodec, String> {
    let mime = if profile.codec.is_h265() {
        "video/hevc"
    } else {
        "video/avc"
    };
    let codec = MediaCodec::from_decoder_type(mime)
        .ok_or_else(|| format!("no {mime} decoder on this device"))?;

    let mut format = MediaFormat::new();
    format.set_str("mime", mime);
    format.set_i32("width", profile.width as i32);
    format.set_i32("height", profile.height as i32);
    // MediaFormat.KEY_LOW_LATENCY (Android 11+): output each frame as soon as
    // it's decoded instead of buffering for smooth playback.
    format.set_i32("low-latency", 1);
    // Realtime priority.
    format.set_i32("priority", 0);
    // Vendor low-latency modes (Qualcomm, MediaTek); ignored elsewhere.
    format.set_i32("vendor.qti-ext-dec-low-latency.enable", 1);
    format.set_i32("vendor.rtc-ext-dec-low-latency.enable", 1);

    codec
        .configure(&format, Some(window), MediaCodecDirection::Decoder)
        .map_err(|e| format!("configure {mime}: {e}"))?;
    codec.start().map_err(|e| format!("start {mime}: {e}"))?;
    Ok(codec)
}

fn decode_loop(
    codec: &MediaCodec,
    profile: &VideoProfile,
    frames: &Receiver<EncodedFrame>,
    on_event: &impl Fn(StreamEvent),
    on_error: &impl Fn(),
) -> DecodeStats {
    let mut stats = DecodeStats {
        decoder: codec.name().unwrap_or_else(|_| "MediaCodec".into()),
        ..Default::default()
    };
    tracing::info!("video decoder: {}", stats.decoder);
    let frame_us = 1_000_000 / u64::from(profile.max_fps.max(1));
    let mut pts_us: u64 = 0;
    let mut started = false;

    while let Ok(frame) = frames.recv() {
        if frame.frames_lost > 0 {
            tracing::debug!("{} frames lost", frame.frames_lost);
        }
        if queue_frame(
            codec,
            &frame.data,
            pts_us,
            frames,
            &mut stats,
            &mut started,
            profile,
            on_event,
        ) {
            stats.queued += 1;
        } else {
            stats.errors += 1;
            on_error();
        }
        pts_us += frame_us;

        // Draw what's ready; wait a little for this frame if nothing is.
        let mut drawn = drain(
            codec,
            frames,
            &mut stats,
            &mut started,
            profile,
            on_event,
            Duration::ZERO,
        );
        let mut tries = 0;
        while drawn == 0 && tries < OUTPUT_TRIES && frames.is_empty() {
            drawn = drain(
                codec,
                frames,
                &mut stats,
                &mut started,
                profile,
                on_event,
                OUTPUT_WAIT,
            );
            tries += 1;
        }
    }
    stats
}

/// Copies one access unit into an input buffer. Returns false if it had to
/// be dropped.
#[allow(clippy::too_many_arguments)]
fn queue_frame(
    codec: &MediaCodec,
    data: &[u8],
    pts_us: u64,
    frames: &Receiver<EncodedFrame>,
    stats: &mut DecodeStats,
    started: &mut bool,
    profile: &VideoProfile,
    on_event: &impl Fn(StreamEvent),
) -> bool {
    for _ in 0..INPUT_TRIES {
        match codec.dequeue_input_buffer(INPUT_WAIT) {
            Ok(DequeuedInputBufferResult::Buffer(mut buf)) => {
                let dst = buf.buffer_mut();
                if data.len() > dst.len() {
                    tracing::warn!(
                        "frame of {} bytes doesn't fit a {} byte input buffer",
                        data.len(),
                        dst.len()
                    );
                    let _ = codec.queue_input_buffer(buf, 0, 0, pts_us, 0);
                    return false;
                }
                // SAFETY: `dst` has room for `data.len()` bytes; the regions
                // don't overlap.
                unsafe {
                    std::ptr::copy_nonoverlapping(
                        data.as_ptr(),
                        dst.as_mut_ptr().cast::<u8>(),
                        data.len(),
                    );
                }
                return match codec.queue_input_buffer(buf, 0, data.len(), pts_us, 0) {
                    Ok(()) => true,
                    Err(e) => {
                        tracing::warn!("queue input: {e}");
                        false
                    }
                };
            }
            // All input buffers busy: free some by draining output.
            Ok(DequeuedInputBufferResult::TryAgainLater) => {
                drain(
                    codec,
                    frames,
                    stats,
                    started,
                    profile,
                    on_event,
                    Duration::ZERO,
                );
            }
            Err(e) => {
                tracing::warn!("dequeue input: {e}");
                return false;
            }
        }
    }
    tracing::debug!("no free input buffer; dropping a frame");
    false
}

/// Releases every decoded frame that's ready, drawing it only if no newer
/// encoded frame is waiting (so video never falls behind the console).
/// Returns how many were drawn.
fn drain(
    codec: &MediaCodec,
    frames: &Receiver<EncodedFrame>,
    stats: &mut DecodeStats,
    started: &mut bool,
    profile: &VideoProfile,
    on_event: &impl Fn(StreamEvent),
    first_wait: Duration,
) -> u32 {
    let mut drawn = 0;
    let mut wait = first_wait;
    loop {
        match codec.dequeue_output_buffer(wait) {
            Ok(DequeuedOutputBufferInfoResult::Buffer(buf)) => {
                let render = frames.is_empty();
                if let Err(e) = codec.release_output_buffer(buf, render) {
                    tracing::warn!("release output: {e}");
                    stats.errors += 1;
                } else if render {
                    stats.shown += 1;
                    drawn += 1;
                    if !*started {
                        *started = true;
                        on_event(StreamEvent::VideoStarted {
                            width: profile.width,
                            height: profile.height,
                            decoder: stats.decoder.clone(),
                        });
                    }
                } else {
                    stats.dropped += 1;
                }
            }
            Ok(DequeuedOutputBufferInfoResult::OutputFormatChanged) => {
                tracing::info!("decoder output format changed");
            }
            Ok(DequeuedOutputBufferInfoResult::OutputBuffersChanged) => {}
            Ok(DequeuedOutputBufferInfoResult::TryAgainLater) => return drawn,
            Err(e) => {
                tracing::warn!("dequeue output: {e}");
                stats.errors += 1;
                return drawn;
            }
        }
        wait = Duration::ZERO;
    }
}
