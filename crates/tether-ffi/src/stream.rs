//! A streaming session for the Android app: wake the console, start the
//! session and feed its video to the MediaCodec decoder (see [`crate::video`]).
//!
//! Everything runs on a `stream` thread; Kotlin hears about it through
//! [`StreamListener`] and controls it through [`StreamHandle`].

// The session code only exists for Android; on other targets (desktop CI)
// `run` is a stub and these helpers go unused.
#![cfg_attr(not(target_os = "android"), allow(dead_code, unused_imports))]

use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender};
use rp_core::{AccountId, Codec, Resolution, SessionEvent, VideoProfile};
use tether_core::{ConsoleSecrets, PairedConsole, WakeProgress};

use crate::TetherError;

const WAKE_TIMEOUT: Duration = Duration::from_secs(60);

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
    /// The session ended. `error` is false for a normal stop.
    Ended { reason: String, error: bool },
}

/// Receives stream events, on a background thread.
#[uniffi::export(with_foreign)]
pub trait StreamListener: Send + Sync {
    fn on_event(&self, event: StreamEvent);
}

enum Control {
    LoginPin(String),
    Stop,
}

/// A running stream. Call [`StreamHandle::stop`] to end it.
#[derive(uniffi::Object)]
pub struct StreamHandle {
    ctl: Sender<Control>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

#[uniffi::export]
impl StreamHandle {
    /// Answers a `LoginPinRequest`.
    pub fn set_login_pin(&self, pin: String) {
        let _ = self.ctl.send(Control::LoginPin(pin));
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

pub(crate) fn start(
    paired: PairedConsole,
    secrets: ConsoleSecrets,
    account_id: AccountId,
    options: StreamOptions,
    listener: Arc<dyn StreamListener>,
) -> Result<Arc<StreamHandle>, TetherError> {
    let (ctl, ctl_rx) = crossbeam_channel::unbounded();
    let thread = std::thread::Builder::new()
        .name("stream".into())
        .spawn(move || {
            let (reason, error) = run(&paired, &secrets, account_id, &options, &ctl_rx, &listener);
            listener.on_event(StreamEvent::Ended { reason, error });
        })?;
    Ok(Arc::new(StreamHandle {
        ctl,
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
    listener: &Arc<dyn StreamListener>,
) -> (String, bool) {
    use rp_core::{Session, SessionConfig, VideoSample};

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
    let decoder = match video::spawn_decoder(profile, window, frame_rx, on_started, move || {
        let _ = idr_tx.try_send(());
    }) {
        Ok(d) => d,
        Err(e) => return (format!("video decoder: {e}"), true),
    };

    // 4. Session.
    let config = SessionConfig {
        host: console.addr.to_string(),
        target: paired.target(),
        regist_key: secrets.regist_key,
        rp_key: secrets.rp_key,
        account_id,
        video: profile,
    };
    let sink: rp_core::VideoSink = Box::new(move |sample: VideoSample<'_>| {
        frame_tx
            .send(EncodedFrame {
                data: sample.data.to_vec(),
                frames_lost: sample.frames_lost,
            })
            .is_ok()
    });
    let (session, events) = match Session::start(&config, sink) {
        Ok(s) => s,
        Err(e) => return (e.to_string(), true),
    };

    let mut stop_deadline: Option<Instant> = None;
    let mut quit: Option<(String, bool)> = None;
    while quit.is_none() {
        let timeout = stop_deadline
            .map(|d| d.saturating_duration_since(Instant::now()))
            .unwrap_or(Duration::from_secs(3600));
        crossbeam_channel::select! {
            recv(events) -> ev => match ev {
                Ok(SessionEvent::Connected) => listener.on_event(StreamEvent::Connected),
                Ok(SessionEvent::LoginPinRequest { previous_incorrect }) => {
                    listener.on_event(StreamEvent::LoginPinRequest { previous_incorrect });
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
                Ok(Control::Stop) | Err(_) => {
                    if stop_deadline.is_none() {
                        session.stop();
                        stop_deadline = Some(Instant::now() + Duration::from_secs(5));
                    }
                }
            },
            recv(idr_rx) -> _ => { let _ = session.request_idr(); },
            default(timeout) => {
                if stop_deadline.is_some() {
                    quit = Some(("Disconnected".into(), false));
                }
            }
        }
    }

    // Dropping the session joins chiaki and closes the frame channel, which
    // ends the decoder thread.
    drop(session);
    if let Ok(stats) = decoder.join() {
        tracing::info!(
            "video: {} queued, {} shown, {} dropped (decoder behind), {} errors; {}",
            stats.queued,
            stats.shown,
            stats.dropped,
            stats.errors,
            stats.decoder
        );
    }
    let (mut reason, error) = quit.unwrap();
    if stop_deadline.is_some() {
        reason = "Disconnected".into();
    }
    (reason, error)
}
