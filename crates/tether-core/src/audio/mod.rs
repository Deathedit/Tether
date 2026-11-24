//! Session audio: plays the console's sound and captures the microphone.
//!
//! The console sends Opus (48 kHz stereo, 10 ms frames). [`AudioPlayer`]
//! decodes it on chiaki's network thread, converts it to the output device's
//! format if needed and queues it in a small [`JitterBuffer`] that the
//! device's callback drains. Output goes through `cpal`: WASAPI on Windows,
//! ALSA (and through it PipeWire or PulseAudio) on Linux, AAudio on Android.

mod buffer;
mod convert;
mod mic;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample};
use rp_core::{AudioData, AudioFormat, AudioSink};

pub use buffer::{BufferStats, JitterBuffer};
pub use convert::Converter;
pub use mic::Microphone;

#[derive(Debug, thiserror::Error)]
pub enum AudioError {
    #[error("no audio {0} device")]
    NoDevice(&'static str),
    #[error("audio device: {0}")]
    Device(#[from] cpal::Error),
    #[error("opus: {0}")]
    Opus(#[from] opus::Error),
    #[error("audio device has an unsupported sample format ({0})")]
    UnsupportedFormat(SampleFormat),
}

/// What the console sends; the output stream is opened for this up front.
const STREAM_RATE: u32 = 48_000;
const STREAM_CHANNELS: u16 = 2;

#[derive(Debug, Clone, Copy)]
pub struct AudioOptions {
    /// Audio buffered before playback starts (and kept after an underrun),
    /// on top of the device's own buffer. Lower means less delay but more
    /// risk of gaps on a jittery network.
    pub buffer: Duration,
}

impl Default for AudioOptions {
    fn default() -> Self {
        Self {
            buffer: Duration::from_millis(30),
        }
    }
}

/// Plays a session's audio on the default output device. Hand
/// [`AudioPlayer::sink`] to the session; dropping the player stops playback.
pub struct AudioPlayer {
    shared: Arc<Shared>,
    _stream: cpal::Stream,
    device_name: String,
    device_rate: u32,
    device_channels: u16,
}

struct Shared {
    buffer: Mutex<JitterBuffer>,
    device_rate: u32,
    device_channels: usize,
    target_frames: usize,
}

impl AudioPlayer {
    pub fn new(options: AudioOptions) -> Result<Self, AudioError> {
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or(AudioError::NoDevice("output"))?;
        let device_name = device
            .description()
            .map(|d| d.name().to_owned())
            .unwrap_or_else(|_| "default".into());
        let (format, mut config) = output_config(&device)?;

        // Ask for a 10 ms device buffer (one console frame) when allowed.
        let period = config.sample_rate / 100;
        if let cpal::SupportedBufferSize::Range { min, max } = format.buffer_size()
            && (*min..=*max).contains(&period)
        {
            config.buffer_size = cpal::BufferSize::Fixed(period);
        }

        let device_rate = config.sample_rate;
        let device_channels = config.channels;
        let target = (options.buffer.as_secs_f64() * device_rate as f64).round() as usize;
        // Allow bursts of up to 100 ms more before dropping audio.
        let max = target + device_rate as usize / 10;
        let shared = Arc::new(Shared {
            buffer: Mutex::new(JitterBuffer::new(device_channels as usize, target, max)),
            device_rate,
            device_channels: device_channels as usize,
            target_frames: target,
        });

        let stream = match format.sample_format() {
            SampleFormat::F32 => build_output::<f32>(&device, config, shared.clone()),
            SampleFormat::I16 => build_output::<i16>(&device, config, shared.clone()),
            SampleFormat::I32 => build_output::<i32>(&device, config, shared.clone()),
            SampleFormat::U16 => build_output::<u16>(&device, config, shared.clone()),
            other => return Err(AudioError::UnsupportedFormat(other)),
        }?;
        stream.play()?;
        tracing::info!(
            "audio out: {device_name}, {device_rate} Hz, {device_channels} ch, {:?}, buffer {target} frames",
            format.sample_format()
        );
        Ok(Self {
            shared,
            _stream: stream,
            device_name,
            device_rate,
            device_channels,
        })
    }

    pub fn device_name(&self) -> &str {
        &self.device_name
    }

    pub fn device_format(&self) -> (u32, u16) {
        (self.device_rate, self.device_channels)
    }

    pub fn stats(&self) -> BufferStats {
        self.shared.buffer.lock().unwrap().stats()
    }

    /// The callback for `rp_core::Sinks::audio`: decodes Opus and queues it.
    pub fn sink(&self) -> AudioSink {
        let shared = self.shared.clone();
        let mut decoder: Option<StreamDecoder> = None;
        Box::new(move |data: AudioData<'_>| match data {
            AudioData::Format(format) => match StreamDecoder::new(format, &shared) {
                Ok(d) => {
                    tracing::info!(
                        "audio in: {} Hz, {} ch, {} samples per frame",
                        format.rate,
                        format.channels,
                        format.frame_size
                    );
                    shared.buffer.lock().unwrap().clear();
                    decoder = Some(d);
                }
                Err(e) => {
                    tracing::warn!("can't decode the console's audio ({format:?}): {e}");
                    decoder = None;
                }
            },
            AudioData::Frame(packet) => {
                if let Some(d) = &mut decoder {
                    d.decode(packet, &shared);
                }
            }
            AudioData::Lost => {
                if let Some(d) = &mut decoder {
                    d.decode(&[], &shared);
                }
            }
        })
    }
}

/// Opus decoder plus conversion to the device's format.
struct StreamDecoder {
    opus: opus::Decoder,
    channels: usize,
    pcm: Vec<f32>,
    converter: Converter,
    converted: Vec<f32>,
    errors: u64,
}

impl StreamDecoder {
    fn new(format: AudioFormat, shared: &Shared) -> Result<Self, AudioError> {
        let channels = match format.channels {
            1 => opus::Channels::Mono,
            _ => opus::Channels::Stereo,
        };
        let opus = opus::Decoder::new(format.rate, channels)?;
        Ok(Self {
            opus,
            channels: channels as usize,
            // One frame; Opus never returns more than 120 ms at once.
            pcm: vec![0.0; format.frame_size.max(480) as usize * channels as usize],
            converter: Converter::new(
                channels as usize,
                format.rate,
                shared.device_channels,
                shared.device_rate,
            ),
            converted: Vec::new(),
            errors: 0,
        })
    }

    /// Decodes one packet (`&[]` conceals a lost one) and queues it.
    fn decode(&mut self, packet: &[u8], shared: &Shared) {
        let samples = match self.opus.decode_float(packet, &mut self.pcm, false) {
            Ok(n) => n,
            Err(e) => {
                self.errors += 1;
                if self.errors.is_power_of_two() {
                    tracing::warn!("opus decode failed ({} so far): {e}", self.errors);
                }
                return;
            }
        };
        let pcm = &self.pcm[..samples * self.channels];
        self.converted.clear();
        self.converter.process(pcm, &mut self.converted);
        shared.buffer.lock().unwrap().push(&self.converted);
    }
}

/// Picks the output format: the console's 48 kHz stereo if the device takes
/// it (WASAPI converts anything, PipeWire usually runs at 48 kHz), else the
/// device's default.
fn output_config(
    device: &cpal::Device,
) -> Result<(cpal::SupportedStreamConfig, cpal::StreamConfig), AudioError> {
    let preferred = device.supported_output_configs().ok().and_then(|configs| {
        let mut matching: Vec<_> = configs
            .filter(|c| c.channels() == STREAM_CHANNELS)
            .filter_map(|c| c.try_with_sample_rate(STREAM_RATE))
            .collect();
        // f32 first: no conversion in the callback.
        matching.sort_by_key(|c| c.sample_format() != SampleFormat::F32);
        matching.into_iter().next()
    });
    let format = match preferred {
        Some(f) => f,
        None => device.default_output_config()?,
    };
    let config = format.config();
    Ok((format, config))
}

fn build_output<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    shared: Arc<Shared>,
) -> Result<cpal::Stream, AudioError>
where
    T: SizedSample + FromSample<f32>,
{
    let mut scratch: Vec<f32> = Vec::new();
    let mut reported = BufferStats::default();
    let stream = device.build_output_stream(
        config,
        move |out: &mut [T], _: &cpal::OutputCallbackInfo| {
            scratch.resize(out.len(), 0.0);
            let stats = {
                let mut buffer = shared.buffer.lock().unwrap();
                buffer.pop(&mut scratch);
                buffer.stats()
            };
            for (dst, &src) in out.iter_mut().zip(scratch.iter()) {
                *dst = T::from_sample(src);
            }
            if stats.underruns != reported.underruns {
                tracing::debug!(
                    "audio underrun (total {}); rebuffering {} frames",
                    stats.underruns,
                    shared.target_frames
                );
            }
            if stats.dropped_frames != reported.dropped_frames {
                tracing::debug!(
                    "audio backlog: dropped {} frames (total {})",
                    stats.dropped_frames - reported.dropped_frames,
                    stats.dropped_frames
                );
            }
            reported = stats;
        },
        |e| tracing::warn!("audio output: {e}"),
        None,
    )?;
    Ok(stream)
}
