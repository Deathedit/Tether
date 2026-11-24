//! Video decoding with FFmpeg, off the UI thread.
//!
//! Encoded frames from the session go in. Each decoded frame ends up as NV12
//! in system memory (hardware frames are downloaded, other software formats
//! go through swscale) and is handed to `on_frame`, which uploads it to the
//! GPU ([`render::VideoRenderer`]).
//!
//! To never fall behind the console, every frame is decoded (the next ones
//! depend on it) but only the newest is downloaded, converted and shown when
//! more encoded frames are already waiting. The UI coalesces redraws on its
//! side.

pub mod hwaccel;
pub mod render;

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::thread::JoinHandle;

use anyhow::{Context, Result};
use crossbeam_channel::Receiver;
use ffmpeg::format::Pixel;
use ffmpeg::software::scaling;
use ffmpeg_next as ffmpeg;
use rp_core::Codec;

pub use hwaccel::Backend;
use render::{ColorInfo, Nv12};

/// After this many hardware failures in a row with no good frame, the
/// decoder is reopened in software.
const HW_FAILURES_BEFORE_FALLBACK: u32 = 15;

pub struct EncodedFrame {
    pub data: Vec<u8>,
    pub frames_lost: i32,
}

/// Counters the UI reads a few times a second.
#[derive(Default)]
pub struct VideoStats {
    pub decoded: AtomicU64,
    /// Decoded frames that were shown.
    pub presented: AtomicU64,
    /// Decoded frames dropped unshown because newer ones were already
    /// queued (the decoder was behind).
    pub skipped: AtomicU64,
    pub errors: AtomicU64,
    pub bytes: AtomicU64,
    pub frames_lost: AtomicU64,
    backend: AtomicU8,
}

impl VideoStats {
    pub fn backend(&self) -> Backend {
        Backend::from_u8(self.backend.load(Ordering::Relaxed))
    }

    fn set_backend(&self, b: Backend) {
        self.backend.store(b.as_u8(), Ordering::Relaxed);
    }
}

#[derive(Debug, Clone, Copy)]
pub struct DecoderOptions {
    pub codec: Codec,
    /// Try the platform's hardware decoder first.
    pub hardware: bool,
}

/// Starts the decoder thread. It ends when `frames` is closed (the session
/// is gone). `on_frame` gets each decoded frame; `on_error` runs when the
/// decoder needs a keyframe.
pub fn spawn_decoder(
    opts: DecoderOptions,
    frames: Receiver<EncodedFrame>,
    stats: Arc<VideoStats>,
    mut on_frame: impl FnMut(&Nv12<'_>) + Send + 'static,
    on_error: impl Fn() + Send + 'static,
) -> Result<JoinHandle<()>> {
    ffmpeg::init().context("FFmpeg init failed")?;
    let mut decoder = Decoder::open(opts.codec, opts.hardware)?;
    stats.set_backend(decoder.backend);
    tracing::info!("video decoder: {}", decoder.backend);

    let handle = std::thread::Builder::new()
        .name("video-decode".into())
        .spawn(move || {
            let mut decoded = ffmpeg::frame::Video::empty();
            let mut downloaded = ffmpeg::frame::Video::empty();
            let mut converted = ffmpeg::frame::Video::empty();
            let mut scaler: Option<(scaling::Context, (Pixel, u32, u32))> = None;
            let mut hw_failures = 0u32;

            for frame in frames.iter() {
                stats
                    .bytes
                    .fetch_add(frame.data.len() as u64, Ordering::Relaxed);
                if frame.frames_lost > 0 {
                    stats
                        .frames_lost
                        .fetch_add(frame.frames_lost as u64, Ordering::Relaxed);
                }

                if decoder.backend != Backend::Software
                    && hw_failures >= HW_FAILURES_BEFORE_FALLBACK
                {
                    tracing::warn!(
                        "{} keeps failing; switching to software decoding",
                        decoder.backend
                    );
                    match Decoder::open(opts.codec, false) {
                        Ok(d) => {
                            decoder = d;
                            stats.set_backend(Backend::Software);
                            on_error();
                        }
                        Err(e) => tracing::error!("could not open software decoder: {e:#}"),
                    }
                    hw_failures = 0;
                }

                if let Err(e) = decoder
                    .inner
                    .send_packet(&ffmpeg::Packet::copy(&frame.data))
                {
                    stats.errors.fetch_add(1, Ordering::Relaxed);
                    tracing::debug!("decoder rejected frame: {e}");
                    if decoder.backend != Backend::Software {
                        hw_failures += 1;
                    }
                    on_error();
                    continue;
                }
                while decoder.inner.receive_frame(&mut decoded).is_ok() {
                    stats.decoded.fetch_add(1, Ordering::Relaxed);
                    if !frames.is_empty() {
                        // Newer frames are queued: don't spend time showing this one.
                        stats.skipped.fetch_add(1, Ordering::Relaxed);
                        continue;
                    }
                    let mut sw: &ffmpeg::frame::Video = &decoded;
                    if hwaccel::is_hw_frame(&decoded) {
                        if let Err(e) = hwaccel::download(&decoded, &mut downloaded) {
                            stats.errors.fetch_add(1, Ordering::Relaxed);
                            hw_failures += 1;
                            tracing::debug!("could not download frame from the GPU: {e}");
                            continue;
                        }
                        sw = &downloaded;
                        hw_failures = 0;
                        stats.set_backend(decoder.backend);
                    } else {
                        stats.set_backend(Backend::Software);
                    }

                    if sw.format() != Pixel::NV12 {
                        let key = (sw.format(), sw.width(), sw.height());
                        if scaler.as_ref().is_none_or(|(_, k)| *k != key) {
                            match scaling::Context::get(
                                key.0,
                                key.1,
                                key.2,
                                Pixel::NV12,
                                key.1,
                                key.2,
                                scaling::Flags::POINT,
                            ) {
                                Ok(s) => scaler = Some((s, key)),
                                Err(e) => {
                                    tracing::warn!("cannot convert {:?} to NV12: {e}", key.0);
                                    continue;
                                }
                            }
                        }
                        let (s, _) = scaler.as_mut().unwrap();
                        if let Err(e) = s.run(sw, &mut converted) {
                            tracing::warn!("NV12 conversion failed: {e}");
                            continue;
                        }
                        converted.set_color_space(sw.color_space());
                        converted.set_color_range(sw.color_range());
                        sw = &converted;
                    }

                    stats.presented.fetch_add(1, Ordering::Relaxed);
                    on_frame(&Nv12 {
                        width: sw.width(),
                        height: sw.height(),
                        y: sw.data(0),
                        y_stride: sw.stride(0),
                        uv: sw.data(1),
                        uv_stride: sw.stride(1),
                        color: color_info(sw),
                    });
                }
            }
        })?;
    Ok(handle)
}

struct Decoder {
    inner: ffmpeg::decoder::Video,
    backend: Backend,
}

impl Decoder {
    /// Opens an H.264/H.265 decoder, with the platform's hardware backend if
    /// `hardware` is set and it works.
    fn open(codec: Codec, hardware: bool) -> Result<Self> {
        let id = if codec.is_h265() {
            ffmpeg::codec::Id::HEVC
        } else {
            ffmpeg::codec::Id::H264
        };
        let av_codec =
            ffmpeg::decoder::find(id).with_context(|| format!("FFmpeg has no {id:?} decoder"))?;
        let mut ctx = ffmpeg::codec::Context::new_with_codec(av_codec);
        ctx.set_flags(ffmpeg::codec::Flags::LOW_DELAY);

        let mut backend = Backend::Software;
        if hardware && let Some(hw) = Backend::platform_default() {
            match hwaccel::attach(&mut ctx, &av_codec, hw) {
                Ok(()) => backend = hw,
                Err(e) => tracing::warn!("hardware decoding unavailable: {e}"),
            }
        }
        if backend == Backend::Software {
            // Frame threading buffers several frames (latency); slices don't.
            ctx.set_threading(ffmpeg::codec::threading::Config {
                kind: ffmpeg::codec::threading::Type::Slice,
                count: 0,
            });
        }
        let inner = ctx
            .decoder()
            .video()
            .context("could not open video decoder")?;
        Ok(Self { inner, backend })
    }
}

fn color_info(frame: &ffmpeg::frame::Video) -> ColorInfo {
    use ffmpeg::color::{Range, Space};
    let bt709 = match frame.color_space() {
        Space::BT709 => true,
        Space::BT470BG | Space::SMPTE170M | Space::FCC | Space::SMPTE240M => false,
        // Unspecified: HD streams are BT.709 in practice.
        _ => frame.height() >= 720,
    };
    ColorInfo {
        bt709,
        full_range: frame.color_range() == Range::JPEG,
    }
}
