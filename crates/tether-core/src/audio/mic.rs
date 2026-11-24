//! Microphone capture for the console's voice chat: default input device →
//! 48 kHz stereo → Opus packets of exactly the size chiaki sends.

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, SizedSample};
use rp_core::{MIC_FORMAT, MIC_PACKET_SIZE};

use super::{AudioError, Converter};

/// Captures the default microphone while it exists. Each 10 ms of audio
/// becomes one Opus packet handed to the callback (on the audio thread).
pub struct Microphone {
    _stream: cpal::Stream,
    device_name: String,
}

impl Microphone {
    /// `send` gets every packet; pass them to `Session::send_microphone`.
    pub fn start(send: impl FnMut(&[u8]) + Send + 'static) -> Result<Self, AudioError> {
        let device = cpal::default_host()
            .default_input_device()
            .ok_or(AudioError::NoDevice("input"))?;
        let device_name = device
            .description()
            .map(|d| d.name().to_owned())
            .unwrap_or_else(|_| "default".into());
        let format = device.default_input_config()?;
        let config = format.config();
        let packetizer = Packetizer::new(config.channels as usize, config.sample_rate, send)?;
        let stream = match format.sample_format() {
            SampleFormat::F32 => build_input::<f32>(&device, config, packetizer),
            SampleFormat::I16 => build_input::<i16>(&device, config, packetizer),
            SampleFormat::I32 => build_input::<i32>(&device, config, packetizer),
            SampleFormat::U16 => build_input::<u16>(&device, config, packetizer),
            other => return Err(AudioError::UnsupportedFormat(other)),
        }?;
        stream.play()?;
        tracing::info!(
            "microphone: {device_name}, {} Hz, {} ch",
            config.sample_rate,
            config.channels
        );
        Ok(Self {
            _stream: stream,
            device_name,
        })
    }

    pub fn device_name(&self) -> &str {
        &self.device_name
    }
}

fn build_input<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    mut packetizer: Packetizer,
) -> Result<cpal::Stream, AudioError>
where
    T: SizedSample,
    f32: cpal::FromSample<T>,
{
    let mut scratch = Vec::new();
    let stream = device.build_input_stream(
        config,
        move |data: &[T], _: &cpal::InputCallbackInfo| {
            scratch.clear();
            scratch.extend(data.iter().map(|&s| cpal::Sample::to_sample::<f32>(s)));
            packetizer.push(&scratch);
        },
        |e| tracing::warn!("microphone: {e}"),
        None,
    )?;
    Ok(stream)
}

type PacketSink = Box<dyn FnMut(&[u8]) + Send>;

/// Converts captured audio to the console's format, cuts it into 10 ms
/// frames and encodes each one.
struct Packetizer {
    converter: Converter,
    converted: Vec<f32>,
    /// 48 kHz stereo i16 not yet encoded.
    pending: Vec<i16>,
    encoder: opus::Encoder,
    packet: [u8; MIC_PACKET_SIZE],
    send: PacketSink,
}

impl Packetizer {
    fn new(
        channels: usize,
        rate: u32,
        send: impl FnMut(&[u8]) + Send + 'static,
    ) -> Result<Self, AudioError> {
        Ok(Self {
            converter: Converter::new(
                channels,
                rate,
                MIC_FORMAT.channels as usize,
                MIC_FORMAT.rate,
            ),
            converted: Vec::new(),
            pending: Vec::new(),
            encoder: mic_encoder()?,
            packet: [0; MIC_PACKET_SIZE],
            send: Box::new(send),
        })
    }

    fn push(&mut self, samples: &[f32]) {
        self.converted.clear();
        self.converter.process(samples, &mut self.converted);
        self.pending.extend(
            self.converted
                .iter()
                .map(|&s| (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16),
        );
        let frame = (MIC_FORMAT.frame_size * MIC_FORMAT.channels as u32) as usize;
        let mut start = 0;
        while self.pending.len() - start >= frame {
            let pcm = &self.pending[start..start + frame];
            match self.encoder.encode(pcm, &mut self.packet) {
                Ok(n) if n == MIC_PACKET_SIZE => (self.send)(&self.packet),
                Ok(n) => tracing::trace!("dropping {n}-byte mic packet"),
                Err(e) => tracing::debug!("mic encode failed: {e}"),
            }
            start += frame;
        }
        self.pending.drain(..start);
    }
}

/// chiaki only sends fixed 40-byte packets, so encode at the constant
/// bitrate that fills one exactly (40 bytes per 10 ms = 32 kbit/s), with
/// chiaki-ng's other settings.
fn mic_encoder() -> Result<opus::Encoder, AudioError> {
    let mut enc = opus::Encoder::new(
        MIC_FORMAT.rate,
        opus::Channels::Stereo,
        opus::Application::LowDelay,
    )?;
    let bitrate = (MIC_PACKET_SIZE as u32 * 8 * MIC_FORMAT.rate / MIC_FORMAT.frame_size) as i32;
    enc.set_bitrate(opus::Bitrate::Bits(bitrate))?;
    enc.set_vbr(false)?;
    enc.set_signal(opus::Signal::Music)?;
    enc.set_phase_inversion_disabled(true)?;
    Ok(enc)
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;

    #[test]
    fn packets_are_exactly_chiakis_size() {
        let sizes = Arc::new(Mutex::new(Vec::new()));
        let sink = sizes.clone();
        // A mono 44.1 kHz microphone, fed in odd-sized chunks.
        let mut p = Packetizer::new(1, 44_100, move |pkt: &[u8]| {
            sink.lock().unwrap().push(pkt.len());
        })
        .unwrap();
        let tone: Vec<f32> = (0..44_100)
            .map(|i| (i as f32 * 440.0 * std::f32::consts::TAU / 44_100.0).sin() * 0.5)
            .collect();
        for chunk in tone.chunks(333) {
            p.push(chunk);
        }
        p.push(&vec![0.0; 4410]); // silence too
        let sizes = sizes.lock().unwrap();
        // 1.1 s of audio ≈ 110 packets of 10 ms.
        assert!((105..=110).contains(&sizes.len()), "{}", sizes.len());
        assert!(sizes.iter().all(|&n| n == MIC_PACKET_SIZE));
    }
}
