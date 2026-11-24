//! A streaming session: wake the console, start the session, decode and
//! render its video. Everything here runs on worker threads; the UI hears
//! about it through [`StreamEvent`]s.
//!
//! Threads:
//! - `stream`: wakes the console, owns the [`Session`] and handles its events
//!   and the UI's commands ([`Stream::stop`], [`Stream::set_login_pin`]).
//! - chiaki's own threads: network; video, audio and haptics arrive in the
//!   sinks.
//! - `video-decode`: FFmpeg decode, then NV12 upload and conversion with
//!   wgpu ([`crate::video`]).
//! - sound, controllers and microphone: [`crate::play::Play`] and its
//!   threads (cpal output, SDL3 gamepads, event tap).

use std::net::IpAddr;
use std::sync::{Arc, OnceLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crossbeam_channel::{Receiver, Sender};
use rp_core::{AccountId, Session, SessionConfig, SessionEvent, VideoSample};
use slint::wgpu_30::wgpu;
use tether_core::{
    ConsoleSecrets, DecoderPreference, LastSession, PairedConsole, StreamSettings, WakeProgress,
};

use crate::play::{Play, PlayOptions};
use crate::video::render::VideoRenderer;
use crate::video::{self, DecoderOptions, EncodedFrame, VideoStats};

/// How long to keep waking a console before giving up.
const WAKE_TIMEOUT: Duration = Duration::from_secs(60);

/// The wgpu device and queue the UI renders with. Video textures must be
/// created on the same device. Set once the window has rendered.
#[derive(Clone)]
pub struct Gpu {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
}

pub type SharedGpu = Arc<OnceLock<Gpu>>;

pub struct StreamRequest {
    pub paired: PairedConsole,
    pub secrets: ConsoleSecrets,
    pub account_id: AccountId,
    pub settings: StreamSettings,
    /// Sound, microphone and controller options.
    pub play: PlayOptions,
}

pub enum StreamEvent {
    /// A wake packet went out; waiting for the console to report ready.
    Waking,
    /// The console is on and answered at `addr`.
    Found {
        addr: IpAddr,
        woke: bool,
    },
    /// The session started; handshake in progress.
    Handshake,
    /// The console asks for its Remote Play passcode.
    LoginPinRequest {
        previous_incorrect: bool,
    },
    /// The stream is up; video follows.
    Connected,
    /// A new video texture: the first frame, or the video size changed.
    Texture(wgpu::Texture),
    /// A new frame was drawn into the current texture.
    Frame,
    Ended(StreamEnd),
}

pub struct StreamEnd {
    pub reason: String,
    pub error: bool,
    /// Set if the stream got as far as connecting.
    pub summary: Option<LastSession>,
}

enum Control {
    LoginPin(String),
    Stop,
}

pub type EventSink = Arc<dyn Fn(StreamEvent) + Send + Sync>;

/// A running stream. Dropping it stops the session in the background; use
/// [`Stream::finish`] to wait for it.
pub struct Stream {
    ctl: Sender<Control>,
    stats: Arc<VideoStats>,
    thread: Option<JoinHandle<Option<LastSession>>>,
}

impl Stream {
    pub fn start(
        request: StreamRequest,
        gpu: SharedGpu,
        on_event: EventSink,
    ) -> std::io::Result<Self> {
        let (ctl, ctl_rx) = crossbeam_channel::unbounded();
        let stats = Arc::new(VideoStats::default());
        let thread = {
            let stats = stats.clone();
            std::thread::Builder::new()
                .name("stream".into())
                .spawn(move || {
                    let end = run(request, gpu, ctl_rx, stats, on_event.clone());
                    let summary = end.summary.clone();
                    on_event(StreamEvent::Ended(end));
                    summary
                })?
        };
        Ok(Self {
            ctl,
            stats,
            thread: Some(thread),
        })
    }

    /// Whether the stream thread is done (its `Ended` event has been sent).
    pub fn is_finished(&self) -> bool {
        self.thread.as_ref().is_none_or(|t| t.is_finished())
    }

    /// Stops the stream and waits up to `timeout` for the session to close.
    /// Returns the session summary if it ended in time and had connected.
    pub fn finish(mut self, timeout: Duration) -> Option<LastSession> {
        self.stop();
        let deadline = Instant::now() + timeout;
        while !self.is_finished() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        match self.thread.take() {
            Some(t) if t.is_finished() => t.join().ok().flatten(),
            Some(_) => {
                tracing::warn!("session did not close within {} s", timeout.as_secs());
                None
            }
            None => None,
        }
    }

    pub fn set_login_pin(&self, pin: String) {
        let _ = self.ctl.send(Control::LoginPin(pin));
    }

    pub fn stop(&self) {
        let _ = self.ctl.send(Control::Stop);
    }

    pub fn stats(&self) -> &Arc<VideoStats> {
        &self.stats
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        self.stop();
    }
}

fn ended(reason: impl Into<String>, error: bool) -> StreamEnd {
    StreamEnd {
        reason: reason.into(),
        error,
        summary: None,
    }
}

fn run(
    req: StreamRequest,
    gpu: SharedGpu,
    ctl: Receiver<Control>,
    stats: Arc<VideoStats>,
    on_event: EventSink,
) -> StreamEnd {
    let stopped = |ctl: &Receiver<Control>| ctl.try_iter().any(|c| matches!(c, Control::Stop));

    // 1. Wake the console if needed. This can't be interrupted; a cancel is
    //    noticed as soon as it returns.
    let mut woke = false;
    let console = match tether_core::ensure_awake(&req.paired, &req.secrets, WAKE_TIMEOUT, |p| {
        if p == WakeProgress::Waking && !woke {
            woke = true;
            on_event(StreamEvent::Waking);
        }
    }) {
        Ok(c) => c,
        Err(e) => return ended(e.to_string(), true),
    };
    if stopped(&ctl) {
        return ended("Cancelled", false);
    }
    on_event(StreamEvent::Found {
        addr: console.addr,
        woke,
    });

    // The UI sets the device when the window first renders; that is
    // normally long done by now.
    let deadline = Instant::now() + Duration::from_secs(3);
    while gpu.get().is_none() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    let Some(gpu) = gpu.get().cloned() else {
        return ended("the video renderer isn't ready (no GPU device)", true);
    };

    // 2. Decoder: encoded frames from chiaki's thread -> decode -> wgpu.
    let settings = req.settings;
    let profile = settings.video_profile();
    let (frame_tx, frame_rx) = crossbeam_channel::unbounded::<EncodedFrame>();
    let (idr_tx, idr_rx) = crossbeam_channel::bounded::<()>(1);
    let decoder = {
        let on_event = on_event.clone();
        let mut renderer = VideoRenderer::new(gpu.device, gpu.queue);
        video::spawn_decoder(
            DecoderOptions {
                codec: profile.codec,
                hardware: settings.decoder == DecoderPreference::Auto,
            },
            frame_rx,
            stats.clone(),
            move |frame| match renderer.render(frame) {
                Some(texture) => on_event(StreamEvent::Texture(texture)),
                None => on_event(StreamEvent::Frame),
            },
            move || {
                let _ = idr_tx.try_send(());
            },
        )
    };
    let decoder = match decoder {
        Ok(d) => d,
        Err(e) => return ended(format!("{e:#}"), true),
    };

    // 3. Session.
    let config = SessionConfig {
        host: console.addr.to_string(),
        target: req.paired.target(),
        regist_key: req.secrets.regist_key,
        rp_key: req.secrets.rp_key,
        account_id: req.account_id,
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
    // Sound, microphone, controllers and rumble. Opened before the session so
    // its audio and haptics sinks exist from the first packet.
    let mut play = Play::new(&req.play);
    let (session, events) = match Session::start_with(&config, play.sinks(sink)) {
        Ok(s) => s,
        Err(e) => {
            play.stop();
            return ended(e.to_string(), true);
        }
    };
    let session = Arc::new(session);
    // `Play` reacts to rumble and other feedback events, then passes every
    // event on to the loop below.
    let events = play.attach(session.clone(), events);
    on_event(StreamEvent::Handshake);

    let mut connected_at: Option<(Instant, u64)> = None;
    let mut stop_deadline: Option<Instant> = None;
    let mut quit: Option<(String, bool)> = None;
    while quit.is_none() {
        let timeout = stop_deadline
            .map(|d| d.saturating_duration_since(Instant::now()))
            .unwrap_or(Duration::from_secs(3600));
        crossbeam_channel::select! {
            recv(events) -> ev => match ev {
                Ok(SessionEvent::Connected) => {
                    connected_at = Some((Instant::now(), unix_now()));
                    on_event(StreamEvent::Connected);
                }
                Ok(SessionEvent::LoginPinRequest { previous_incorrect }) => {
                    on_event(StreamEvent::LoginPinRequest { previous_incorrect });
                }
                Ok(SessionEvent::Quit { reason, error }) => {
                    // After our own stop, whatever chiaki reports isn't an error.
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
            recv(idr_rx) -> _ => {
                let _ = session.request_idr();
            },
            default(timeout) => {
                if stop_deadline.is_some() {
                    quit = Some(("Disconnected".into(), false));
                }
            }
        }
    }

    // `Play` releases its session handles (controller thread, microphone);
    // then dropping the last one joins chiaki's threads and closes the frame
    // channel, which ends the decoder thread.
    let stopping = Instant::now();
    play.stop();
    drop(session);
    let _ = decoder.join();
    {
        use std::sync::atomic::Ordering::Relaxed;
        tracing::info!(
            "video: {} decoded, {} shown, {} skipped (decoder behind), {} errors, {} frames lost; {} decoder; shutdown took {} ms",
            stats.decoded.load(Relaxed),
            stats.presented.load(Relaxed),
            stats.skipped.load(Relaxed),
            stats.errors.load(Relaxed),
            stats.frames_lost.load(Relaxed),
            stats.backend(),
            stopping.elapsed().as_millis()
        );
    }

    let (mut reason, error) = quit.unwrap();
    if stop_deadline.is_some() {
        reason = "Disconnected".into();
    }
    let summary = connected_at.map(|(at, started_at)| LastSession {
        console: req.paired.name.clone(),
        started_at,
        duration_secs: at.elapsed().as_secs(),
        video: format!("{}{}", settings.resolution, settings.fps()),
        frames_lost: stats.frames_lost.load(std::sync::atomic::Ordering::Relaxed),
    });
    StreamEnd {
        reason,
        error,
        summary,
    }
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
