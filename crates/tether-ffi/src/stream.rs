//! A streaming session for the Android app: wake the console, start the
//! session, decode its video with MediaCodec (see [`crate::video`]), play its
//! sound and take controller and touchpad input.
//!
//! Everything runs on a `stream` thread; Kotlin hears about it through
//! [`StreamListener`] and controls it through [`StreamHandle`].

// The session code only exists for Android; on other targets (desktop CI)
// `run` is a stub and these helpers go unused.
#![cfg_attr(not(target_os = "android"), allow(dead_code, unused_imports))]

use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender};
use rp_core::{AccountId, Buttons, Codec, ControllerState, Resolution, SessionEvent, VideoProfile};
use tether_core::{ConsoleSecrets, PairedConsole, WakeProgress};

use crate::TetherError;

const WAKE_TIMEOUT: Duration = Duration::from_secs(60);
/// DualSense touchpad resolution, as the console expects it.
const TOUCHPAD_MAX_X: f32 = 1919.0;
const TOUCHPAD_MAX_Y: f32 = 1079.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum VideoResolution {
    P360,
    P540,
    P720,
    P1080,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum VideoCodec {
    H264,
    H265,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct StreamOptions {
    pub resolution: VideoResolution,
    /// 30 or 60.
    pub fps: u32,
    pub codec: VideoCodec,
    /// Play the console's sound.
    pub audio: bool,
}

impl StreamOptions {
    fn video_profile(&self) -> VideoProfile {
        let resolution = match self.resolution {
            VideoResolution::P360 => Resolution::P360,
            VideoResolution::P540 => Resolution::P540,
            VideoResolution::P720 => Resolution::P720,
            VideoResolution::P1080 => Resolution::P1080,
        };
        let codec = match self.codec {
            VideoCodec::H264 => Codec::H264,
            VideoCodec::H265 => Codec::H265,
        };
        VideoProfile::preset(resolution, self.fps, codec)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum StreamEvent {
    /// A wake packet went out; waiting for the console.
    Waking,
    /// The session is starting (console is on).
    Connecting,
    /// The console asks for its Remote Play passcode.
    LoginPinRequest { previous_incorrect: bool },
    /// The stream is up.
    Connected,
    /// The first video frame was drawn.
    VideoStarted {
        width: u32,
        height: u32,
        decoder: String,
    },
    /// Controller rumble: low- and high-frequency motor strength, 0-255.
    Rumble { left: u8, right: u8 },
    /// The microphone turned on or off (or failed to turn on).
    Microphone { on: bool, error: Option<String> },
    /// The session ended. `error` is false for a normal stop.
    Ended { reason: String, error: bool },
}

/// Receives stream events, on a background thread.
#[uniffi::export(with_foreign)]
pub trait StreamListener: Send + Sync {
    fn on_event(&self, event: StreamEvent);
}

/// Controller buttons, as bits of [`ControllerInput::buttons`]. Same values
/// as chiaki's (and the console's) DualSense buttons.
#[derive(Debug, Clone, uniffi::Record)]
pub struct ButtonBits {
    pub cross: u32,
    pub circle: u32,
    pub square: u32,
    pub triangle: u32,
    pub dpad_left: u32,
    pub dpad_right: u32,
    pub dpad_up: u32,
    pub dpad_down: u32,
    pub l1: u32,
    pub r1: u32,
    pub l3: u32,
    pub r3: u32,
    pub options: u32,
    /// "Create".
    pub share: u32,
    pub touchpad: u32,
    pub home: u32,
}

/// The bit values for [`ControllerInput::buttons`].
#[uniffi::export]
pub fn button_bits() -> ButtonBits {
    ButtonBits {
        cross: Buttons::CROSS.0,
        circle: Buttons::CIRCLE.0,
        square: Buttons::SQUARE.0,
        triangle: Buttons::TRIANGLE.0,
        dpad_left: Buttons::DPAD_LEFT.0,
        dpad_right: Buttons::DPAD_RIGHT.0,
        dpad_up: Buttons::DPAD_UP.0,
        dpad_down: Buttons::DPAD_DOWN.0,
        l1: Buttons::L1.0,
        r1: Buttons::R1.0,
        l3: Buttons::L3.0,
        r3: Buttons::R3.0,
        options: Buttons::OPTIONS.0,
        share: Buttons::SHARE.0,
        touchpad: Buttons::TOUCHPAD.0,
        home: Buttons::HOME.0,
    }
}

/// Gamepad state from Android (the Thor's controls or a paired controller).
#[derive(Debug, Clone, Copy, Default, uniffi::Record)]
pub struct ControllerInput {
    /// OR of [`button_bits`] values.
    pub buttons: u32,
    /// Triggers, 0-255.
    pub l2: u8,
    pub r2: u8,
    /// Sticks, -32768..=32767; +x right, +y down.
    pub left_x: i16,
    pub left_y: i16,
    pub right_x: i16,
    pub right_y: i16,
}

/// Live numbers for the companion screen.
#[derive(Debug, Clone, uniffi::Record)]
pub struct StreamStats {
    /// Network round trip; 0 until measured.
    pub latency_ms: u32,
    pub bitrate_mbps: f32,
    /// Frames drawn per second.
    pub fps: f32,
    /// Share of frames lost on the network since the last call.
    pub loss_percent: f32,
    pub decoder: String,
}

/// Live decoder counters, read by `StreamHandle::stats`.
#[derive(Default)]
pub struct VideoCounters {
    /// Frames handed to the decoder.
    pub queued: AtomicU64,
    /// Decoded frames drawn on the surface.
    pub shown: AtomicU64,
    /// Decoded but not drawn because newer frames were already waiting.
    pub dropped: AtomicU64,
    pub errors: AtomicU64,
    /// Encoded bytes received.
    pub bytes: AtomicU64,
    /// Frames the network lost (reported by chiaki).
    pub frames_lost: AtomicU64,
    /// The codec MediaCodec picked, e.g. "c2.qti.avc.decoder".
    pub decoder: Mutex<String>,
}

/// State shared between [`StreamHandle`] and the stream thread.
#[derive(Default)]
struct Live {
    /// Gamepad part of the controller state (buttons, triggers, sticks).
    pad: Mutex<ControllerInput>,
    /// Touchpad part (fingers on the bottom screen's touchpad).
    touch: Mutex<ControllerState>,
    counters: Arc<VideoCounters>,
    rtt_us: AtomicU64,
    last: Mutex<Option<Snapshot>>,
}

#[derive(Clone, Copy)]
struct Snapshot {
    at: Instant,
    bytes: u64,
    shown: u64,
    queued: u64,
    lost: u64,
}

impl Live {
    /// The controller state to send: gamepad + touchpad.
    fn controller_state(&self) -> ControllerState {
        let pad = *self.pad.lock().unwrap();
        let mut state = *self.touch.lock().unwrap();
        state.buttons = Buttons(pad.buttons | (state.buttons.0 & Buttons::TOUCHPAD.0));
        state.l2 = pad.l2;
        state.r2 = pad.r2;
        state.left_x = pad.left_x;
        state.left_y = pad.left_y;
        state.right_x = pad.right_x;
        state.right_y = pad.right_y;
        state
    }

    fn snapshot(&self, at: Instant) -> Snapshot {
        let c = &self.counters;
        Snapshot {
            at,
            bytes: c.bytes.load(Relaxed),
            shown: c.shown.load(Relaxed),
            queued: c.queued.load(Relaxed),
            lost: c.frames_lost.load(Relaxed),
        }
    }
}

enum Control {
    LoginPin(String),
    /// The controller state changed; send it.
    Controller,
    Microphone(bool),
    Stop,
}

/// A running stream. Call [`StreamHandle::stop`] to end it.
#[derive(uniffi::Object)]
pub struct StreamHandle {
    ctl: Sender<Control>,
    live: Arc<Live>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

#[uniffi::export]
impl StreamHandle {
    /// Answers a `LoginPinRequest`.
    pub fn set_login_pin(&self, pin: String) {
        let _ = self.ctl.send(Control::LoginPin(pin));
    }

    /// New gamepad state; sent to the console right away.
    pub fn set_controller(&self, input: ControllerInput) {
        *self.live.pad.lock().unwrap() = input;
        let _ = self.ctl.send(Control::Controller);
    }

    /// A finger touched the touchpad at (`x`, `y`), each 0.0-1.0. Returns
    /// its id for [`StreamHandle::touch_move`] / [`StreamHandle::touch_up`],
    /// or -1 if two fingers are already down.
    pub fn touch_down(&self, x: f32, y: f32) -> i32 {
        let (tx, ty) = touchpad_point(x, y);
        let id = self.live.touch.lock().unwrap().start_touch(tx, ty);
        let _ = self.ctl.send(Control::Controller);
        id.map_or(-1, i32::from)
    }

    pub fn touch_move(&self, id: i32, x: f32, y: f32) {
        let Ok(id) = u8::try_from(id) else { return };
        let (tx, ty) = touchpad_point(x, y);
        self.live.touch.lock().unwrap().move_touch(id, tx, ty);
        let _ = self.ctl.send(Control::Controller);
    }

    pub fn touch_up(&self, id: i32) {
        let Ok(id) = u8::try_from(id) else { return };
        self.live.touch.lock().unwrap().stop_touch(id);
        let _ = self.ctl.send(Control::Controller);
    }

    /// Presses or releases the touchpad itself (a click).
    pub fn touchpad_click(&self, pressed: bool) {
        self.live
            .touch
            .lock()
            .unwrap()
            .buttons
            .set(Buttons::TOUCHPAD, pressed);
        let _ = self.ctl.send(Control::Controller);
    }

    /// Sends the microphone to the console, or stops. A `Microphone` event
    /// reports the result. Needs the RECORD_AUDIO permission.
    pub fn set_microphone(&self, on: bool) {
        let _ = self.ctl.send(Control::Microphone(on));
    }

    /// Latency, bitrate, frame rate and loss since the previous call.
    pub fn stats(&self) -> StreamStats {
        let now = Instant::now();
        let current = self.live.snapshot(now);
        let previous = self.live.last.lock().unwrap().replace(current);
        let (bitrate_mbps, fps, loss_percent) = match previous {
            Some(p) if now > p.at => {
                let secs = (now - p.at).as_secs_f32();
                let frames = current.queued.saturating_sub(p.queued);
                let lost = current.lost.saturating_sub(p.lost);
                let loss = if frames + lost > 0 {
                    lost as f32 * 100.0 / (frames + lost) as f32
                } else {
                    0.0
                };
                (
                    current.bytes.saturating_sub(p.bytes) as f32 * 8.0 / secs / 1_000_000.0,
                    current.shown.saturating_sub(p.shown) as f32 / secs,
                    loss,
                )
            }
            _ => (0.0, 0.0, 0.0),
        };
        StreamStats {
            latency_ms: (self.live.rtt_us.load(Relaxed) / 1000) as u32,
            bitrate_mbps,
            fps,
            loss_percent,
            decoder: self.live.counters.decoder.lock().unwrap().clone(),
        }
    }

    /// Ends the stream; an `Ended` event follows.
    pub fn stop(&self) {
        let _ = self.ctl.send(Control::Stop);
    }

    /// Stops and waits up to `timeout_ms` for the session to close.
    pub fn stop_and_wait(&self, timeout_ms: u32) {
        self.stop();
        let deadline = Instant::now() + Duration::from_millis(timeout_ms.into());
        let mut thread = self.thread.lock().unwrap();
        while thread.as_ref().is_some_and(|t| !t.is_finished()) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        if thread.as_ref().is_some_and(|t| t.is_finished()) {
            let _ = thread.take().map(JoinHandle::join);
        }
    }
}

impl Drop for StreamHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

/// 0.0-1.0 → DualSense touchpad coordinates.
fn touchpad_point(x: f32, y: f32) -> (u16, u16) {
    (
        (x.clamp(0.0, 1.0) * TOUCHPAD_MAX_X).round() as u16,
        (y.clamp(0.0, 1.0) * TOUCHPAD_MAX_Y).round() as u16,
    )
}

pub(crate) fn start(
    paired: PairedConsole,
    secrets: ConsoleSecrets,
    account_id: AccountId,
    options: StreamOptions,
    listener: Arc<dyn StreamListener>,
) -> Result<Arc<StreamHandle>, TetherError> {
    let (ctl, ctl_rx) = crossbeam_channel::unbounded();
    let live = Arc::new(Live::default());
    let thread = {
        let live = live.clone();
        std::thread::Builder::new()
            .name("stream".into())
            .spawn(move || {
                let (reason, error) = run(
                    &paired, &secrets, account_id, &options, &ctl_rx, &live, &listener,
                );
                listener.on_event(StreamEvent::Ended { reason, error });
            })?
    };
    Ok(Arc::new(StreamHandle {
        ctl,
        live,
        thread: Mutex::new(Some(thread)),
    }))
}

#[cfg(not(target_os = "android"))]
fn run(
    _paired: &PairedConsole,
    _secrets: &ConsoleSecrets,
    _account_id: AccountId,
    _options: &StreamOptions,
    _ctl: &Receiver<Control>,
    _live: &Arc<Live>,
    _listener: &Arc<dyn StreamListener>,
) -> (String, bool) {
    (
        "Streaming from this library needs Android (MediaCodec)".into(),
        true,
    )
}

#[cfg(target_os = "android")]
fn run(
    paired: &PairedConsole,
    secrets: &ConsoleSecrets,
    account_id: AccountId,
    options: &StreamOptions,
    ctl: &Receiver<Control>,
    live: &Arc<Live>,
    listener: &Arc<dyn StreamListener>,
) -> (String, bool) {
    use rp_core::{Session, SessionConfig, Sinks, VideoSample};
    use tether_core::{AudioOptions, AudioPlayer, HapticsRumble, Microphone};

    use crate::video::{self, EncodedFrame};

    // 1. Wake the console if needed.
    let mut woke = false;
    let console = match tether_core::ensure_awake(paired, secrets, WAKE_TIMEOUT, |p| {
        if p == WakeProgress::Waking && !woke {
            woke = true;
            listener.on_event(StreamEvent::Waking);
        }
    }) {
        Ok(c) => c,
        Err(e) => return (e.to_string(), true),
    };
    if ctl.try_iter().any(|c| matches!(c, Control::Stop)) {
        return ("Cancelled".into(), false);
    }
    listener.on_event(StreamEvent::Connecting);

    // 2. The surface must exist before the decoder can start.
    let deadline = Instant::now() + Duration::from_secs(5);
    let window = loop {
        if let Some(w) = crate::surface::current() {
            break w;
        }
        if Instant::now() > deadline {
            return (
                "No video surface (the stream screen isn't showing)".into(),
                true,
            );
        }
        std::thread::sleep(Duration::from_millis(20));
    };

    // 3. Decoder thread: chiaki's network thread -> MediaCodec -> surface.
    let profile = options.video_profile();
    let (frame_tx, frame_rx) = crossbeam_channel::unbounded::<EncodedFrame>();
    let (idr_tx, idr_rx) = crossbeam_channel::bounded::<()>(1);
    let on_started = {
        let listener = listener.clone();
        move |ev: StreamEvent| listener.on_event(ev)
    };
    let decoder = match video::spawn_decoder(
        profile,
        window,
        frame_rx,
        live.counters.clone(),
        on_started,
        move || {
            let _ = idr_tx.try_send(());
        },
    ) {
        Ok(d) => d,
        Err(e) => return (format!("video decoder: {e}"), true),
    };

    // 4. Sound (AAudio through cpal). Failing that, the stream runs silent.
    let audio = if options.audio {
        match AudioPlayer::new(AudioOptions::default()) {
            Ok(a) => Some(a),
            Err(e) => {
                tracing::warn!("no sound: {e}");
                None
            }
        }
    } else {
        None
    };

    // 5. Session.
    let config = SessionConfig {
        host: console.addr.to_string(),
        target: paired.target(),
        regist_key: secrets.regist_key,
        rp_key: secrets.rp_key,
        account_id,
        video: profile,
    };
    let video_sink: rp_core::VideoSink = Box::new(move |sample: VideoSample<'_>| {
        frame_tx
            .send(EncodedFrame {
                data: sample.data.to_vec(),
                frames_lost: sample.frames_lost,
            })
            .is_ok()
    });
    // DualSense haptics arrive as audio; turn them into rumble.
    let haptics_sink: rp_core::HapticsSink = {
        let listener = listener.clone();
        let mut rumble = HapticsRumble::new();
        Box::new(move |samples: &[i16]| {
            if let Some((low, high)) = rumble.push(samples) {
                listener.on_event(StreamEvent::Rumble {
                    left: (low >> 8) as u8,
                    right: (high >> 8) as u8,
                });
            }
        })
    };
    let sinks = Sinks {
        video: video_sink,
        audio: audio.as_ref().map(AudioPlayer::sink),
        haptics: Some(haptics_sink),
    };
    let session = match Session::start_with(&config, sinks) {
        Ok((s, events)) => (Arc::new(s), events),
        Err(e) => return (e.to_string(), true),
    };
    let (session, events) = session;

    let mut mic: Option<Microphone> = None;
    let mut mic_connected = false;
    let mut stop_deadline: Option<Instant> = None;
    let mut quit: Option<(String, bool)> = None;
    while quit.is_none() {
        // Wake at least twice a second to refresh the latency reading.
        let timeout = stop_deadline
            .map(|d| d.saturating_duration_since(Instant::now()))
            .unwrap_or(Duration::from_millis(500));
        crossbeam_channel::select! {
            recv(events) -> ev => match ev {
                Ok(SessionEvent::Connected) => listener.on_event(StreamEvent::Connected),
                Ok(SessionEvent::LoginPinRequest { previous_incorrect }) => {
                    listener.on_event(StreamEvent::LoginPinRequest { previous_incorrect });
                }
                Ok(SessionEvent::Rumble { left, right }) => {
                    listener.on_event(StreamEvent::Rumble { left, right });
                }
                Ok(SessionEvent::Quit { reason, error }) => {
                    quit = Some((reason, error && stop_deadline.is_none()));
                }
                Ok(other) => tracing::debug!("session event: {other:?}"),
                Err(_) => quit = Some(("session closed".into(), stop_deadline.is_none())),
            },
            recv(ctl) -> c => match c {
                Ok(Control::LoginPin(pin)) => {
                    if let Err(e) = session.set_login_pin(&pin) {
                        tracing::warn!("could not send passcode: {e}");
                    }
                }
                Ok(Control::Controller) => {
                    // Coalesce a burst of changes into one update.
                    while let Ok(Control::Controller) = ctl.try_recv() {}
                    if let Err(e) = session.set_controller_state(&live.controller_state()) {
                        tracing::debug!("controller state: {e}");
                    }
                }
                Ok(Control::Microphone(on)) => {
                    let result = set_microphone(&session, &mut mic, &mut mic_connected, on);
                    listener.on_event(StreamEvent::Microphone {
                        on: mic.is_some(),
                        error: result.err(),
                    });
                }
                Ok(Control::Stop) | Err(_) => {
                    if stop_deadline.is_none() {
                        mic = None;
                        session.stop();
                        stop_deadline = Some(Instant::now() + Duration::from_secs(5));
                    }
                }
            },
            recv(idr_rx) -> _ => { let _ = session.request_idr(); },
            default(timeout) => {
                if stop_deadline.is_some_and(|d| Instant::now() >= d) {
                    quit = Some(("Disconnected".into(), false));
                }
            }
        }
        if let Some(rtt) = session.rtt() {
            live.rtt_us.store(rtt.as_micros() as u64, Relaxed);
        }
    }

    // The microphone holds a session handle; then dropping the last one
    // joins chiaki and closes the frame channel, which ends the decoder.
    drop(mic);
    drop(session);
    let _ = decoder.join();
    drop(audio);
    {
        let c = &live.counters;
        tracing::info!(
            "video: {} queued, {} shown, {} dropped (decoder behind), {} errors, {} lost; {}",
            c.queued.load(Relaxed),
            c.shown.load(Relaxed),
            c.dropped.load(Relaxed),
            c.errors.load(Relaxed),
            c.frames_lost.load(Relaxed),
            c.decoder.lock().unwrap()
        );
    }
    let (mut reason, error) = quit.unwrap();
    if stop_deadline.is_some() {
        reason = "Disconnected".into();
    }
    (reason, error)
}

/// Turns the microphone on (capturing and sending to the console) or off.
#[cfg(target_os = "android")]
fn set_microphone(
    session: &Arc<rp_core::Session>,
    mic: &mut Option<tether_core::Microphone>,
    connected: &mut bool,
    on: bool,
) -> Result<(), String> {
    if !on {
        *mic = None;
        let _ = session.toggle_microphone(true);
        return Ok(());
    }
    if mic.is_some() {
        return Ok(());
    }
    let sender = session.clone();
    let capture = tether_core::Microphone::start(move |pkt| {
        sender.send_microphone(pkt);
    })
    .map_err(|e| format!("microphone unavailable: {e}"))?;
    if !*connected {
        session
            .connect_microphone()
            .map_err(|e| format!("the console didn't accept the microphone: {e}"))?;
        *connected = true;
    }
    let _ = session.toggle_microphone(false);
    *mic = Some(capture);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn touchpad_points() {
        assert_eq!(touchpad_point(0.0, 0.0), (0, 0));
        assert_eq!(touchpad_point(1.0, 1.0), (1919, 1079));
        assert_eq!(touchpad_point(0.5, 0.5), (960, 540));
        assert_eq!(touchpad_point(-1.0, 2.0), (0, 1079));
    }

    #[test]
    fn controller_merges_pad_and_touch() {
        let live = Live::default();
        *live.pad.lock().unwrap() = ControllerInput {
            buttons: Buttons::CROSS.0,
            l2: 200,
            left_x: -100,
            ..Default::default()
        };
        {
            let mut t = live.touch.lock().unwrap();
            t.start_touch(10, 20);
            t.buttons.set(Buttons::TOUCHPAD, true);
        }
        let s = live.controller_state();
        assert!(s.buttons.contains(Buttons::CROSS | Buttons::TOUCHPAD));
        assert_eq!((s.l2, s.left_x), (200, -100));
        assert_eq!(s.touches[0].id, Some(0));
    }

    #[test]
    fn stats_rates() {
        let live = Arc::new(Live::default());
        let (ctl, _rx) = crossbeam_channel::unbounded();
        let h = StreamHandle {
            ctl,
            live: live.clone(),
            thread: Mutex::new(None),
        };
        let _ = h.stats();
        live.counters.bytes.store(1_000_000, Relaxed);
        live.counters.queued.store(99, Relaxed);
        live.counters.frames_lost.store(1, Relaxed);
        std::thread::sleep(Duration::from_millis(50));
        let s = h.stats();
        assert!(s.bitrate_mbps > 0.0);
        assert!((s.loss_percent - 1.0).abs() < 0.01);
    }
}
