//! Software video decoding with FFmpeg, off the UI thread.
//!
//! Encoded frames from the session go in; frames scaled to fit the window,
//! as packed 0RGB pixels, come out in a shared slot the window draws from.
//! Milestone 5 replaces this with hardware decoding and a wgpu renderer.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use anyhow::{Context, Result};
use crossbeam_channel::Receiver;
use ffmpeg::format::Pixel;
use ffmpeg::software::scaling;
use ffmpeg_next as ffmpeg;
use rp_core::Codec;

/// What a scaler was built for: source format and size, output size.
type ScalerKey = (Pixel, u32, u32, u32, u32);

pub struct EncodedFrame {
    pub data: Vec<u8>,
    pub frames_lost: i32,
}

/// A decoded frame ready to show: `width * height` pixels, 0x00RRGGBB.
pub struct RgbFrame {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u32>,
}

/// State shared between the decoder thread and the window.
#[derive(Default)]
pub struct Shared {
    /// Latest decoded frame. Kept (not taken) so resizes can redraw it.
    pub latest: Mutex<Option<RgbFrame>>,
    /// Window's inner size in pixels; frames are scaled to fit it.
    pub target: Mutex<(u32, u32)>,
    /// Set when a frame was posted to the window and not drawn yet, so the
    /// decoder sends at most one wake-up per drawn frame.
    pub redraw_pending: AtomicBool,
}

#[derive(Default)]
pub struct DecodeStats {
    pub decoded: u64,
    /// Decoded but not shown because newer frames were already waiting.
    pub skipped: u64,
    pub errors: u64,
}

/// Starts the decoder thread. It ends when `frames` is closed (the session
/// is gone). `on_frame` runs after each new frame; `on_error` when the
/// decoder needs a keyframe.
pub fn spawn_decoder(
    codec: Codec,
    frames: Receiver<EncodedFrame>,
    shared: Arc<Shared>,
    on_frame: impl Fn() + Send + 'static,
    on_error: impl Fn() + Send + 'static,
) -> Result<JoinHandle<DecodeStats>> {
    ffmpeg::init().context("FFmpeg init failed")?;
    let id = if codec.is_h265() {
        ffmpeg::codec::Id::HEVC
    } else {
        ffmpeg::codec::Id::H264
    };
    let av_codec =
        ffmpeg::decoder::find(id).with_context(|| format!("FFmpeg has no {id:?} decoder"))?;
    let mut ctx = ffmpeg::codec::Context::new_with_codec(av_codec);
    // Frame threading buffers several frames (latency); slices don't.
    ctx.set_threading(ffmpeg::codec::threading::Config {
        kind: ffmpeg::codec::threading::Type::Slice,
        count: 0,
    });
    ctx.set_flags(ffmpeg::codec::Flags::LOW_DELAY);
    let mut decoder = ctx
        .decoder()
        .video()
        .context("could not open video decoder")?;

    let handle = std::thread::Builder::new()
        .name("video-decode".into())
        .spawn(move || {
            let mut stats = DecodeStats::default();
            let mut scaler: Option<(scaling::Context, ScalerKey)> = None;
            let mut decoded = ffmpeg::frame::Video::empty();
            let mut scaled = ffmpeg::frame::Video::empty();

            while let Ok(frame) = frames.recv() {
                if frame.frames_lost > 0 {
                    tracing::debug!("{} frames lost", frame.frames_lost);
                }
                if let Err(e) = decoder.send_packet(&ffmpeg::Packet::copy(&frame.data)) {
                    stats.errors += 1;
                    tracing::debug!("decoder rejected frame: {e}");
                    on_error();
                    continue;
                }
                while decoder.receive_frame(&mut decoded).is_ok() {
                    stats.decoded += 1;
                    // Every frame must be decoded (later frames reference it),
                    // but if newer ones are already queued, showing this one
                    // only adds delay: skip scaling and drawing it.
                    if !frames.is_empty() {
                        stats.skipped += 1;
                        continue;
                    }
                    let target = *shared.target.lock().unwrap();
                    let (dw, dh) = fit(decoded.width(), decoded.height(), target);
                    let key = (decoded.format(), decoded.width(), decoded.height(), dw, dh);
                    if scaler.as_ref().is_none_or(|(_, k)| *k != key) {
                        match scaling::Context::get(
                            key.0,
                            key.1,
                            key.2,
                            // B,G,R,0 bytes = softbuffer's 0x00RRGGBB on
                            // little-endian machines, so rows copy as-is.
                            Pixel::BGRZ,
                            dw,
                            dh,
                            scaling::Flags::BILINEAR,
                        ) {
                            Ok(s) => scaler = Some((s, key)),
                            Err(e) => {
                                tracing::warn!("no scaler for {key:?}: {e}");
                                continue;
                            }
                        }
                    }
                    let (s, _) = scaler.as_mut().unwrap();
                    if let Err(e) = s.run(&decoded, &mut scaled) {
                        tracing::warn!("scaling failed: {e}");
                        continue;
                    }
                    *shared.latest.lock().unwrap() = Some(to_rgb_frame(&scaled, dw, dh));
                    if !shared.redraw_pending.swap(true, Ordering::AcqRel) {
                        on_frame();
                    }
                }
            }
            stats
        })?;
    Ok(handle)
}

/// Largest size with the source's aspect ratio that fits in `target`.
fn fit(src_w: u32, src_h: u32, target: (u32, u32)) -> (u32, u32) {
    let (tw, th) = (target.0.max(1), target.1.max(1));
    if src_w == 0 || src_h == 0 {
        return (tw, th);
    }
    let w_for_h = (u64::from(th) * u64::from(src_w) / u64::from(src_h)) as u32;
    let (w, h) = if w_for_h <= tw {
        (w_for_h, th)
    } else {
        (
            tw,
            (u64::from(tw) * u64::from(src_h) / u64::from(src_w)) as u32,
        )
    };
    (w.max(1), h.max(1))
}

/// Copies a BGR0 frame (rows may be padded) into packed 0RGB pixels.
fn to_rgb_frame(frame: &ffmpeg::frame::Video, w: u32, h: u32) -> RgbFrame {
    let stride = frame.stride(0);
    let data = frame.data(0);
    let row_bytes = w as usize * 4;
    let mut pixels = vec![0u32; (w * h) as usize];
    let dst: &mut [u8] = bytemuck::cast_slice_mut(&mut pixels);
    for row in 0..h as usize {
        dst[row * row_bytes..][..row_bytes].copy_from_slice(&data[row * stride..][..row_bytes]);
    }
    if cfg!(target_endian = "big") {
        for p in &mut pixels {
            *p = p.swap_bytes();
        }
    }
    RgbFrame {
        width: w,
        height: h,
        pixels,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_keeps_aspect() {
        assert_eq!(fit(1280, 720, (1280, 720)), (1280, 720));
        assert_eq!(fit(1280, 720, (1000, 1000)), (1000, 562));
        assert_eq!(fit(1280, 720, (2000, 720)), (1280, 720));
        assert_eq!(fit(1920, 1080, (0, 0)), (1, 1));
    }
}
