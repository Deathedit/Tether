//! Everything besides video that makes a session playable: sound, the
//! microphone, controllers and their feedback. `stream.rs` creates a
//! [`Play`] before the session (for the audio and haptics sinks), attaches
//! it once the session exists (which also taps the event channel), and stops
//! it before dropping the session.

use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use crossbeam_channel::{Receiver, Sender};
use rp_core::{Session, SessionEvent, Sinks, VideoSink};
use tether_core::{AudioOptions, AudioPlayer, HapticsRumble, Microphone};

use crate::input::{Feedback, Gamepads};

pub struct PlayOptions {
    /// Play the console's sound.
    pub audio: bool,
    /// Extra audio buffering (jitter buffer), in milliseconds.
    pub audio_buffer_ms: u32,
    /// Send the microphone from the start (otherwise the controller's mute
    /// button turns it on).
    pub mic: bool,
}

pub struct Play {
    audio: Option<AudioPlayer>,
    /// Controller feedback (from session events and haptics) for the
    /// gamepad thread; queued until it starts.
    feedback_tx: Sender<Feedback>,
    feedback_rx: Receiver<Feedback>,
    gamepads: Option<Gamepads>,
    mic: Option<Arc<Mutex<Mic>>>,
    start_mic: bool,
}

impl Play {
    /// Opens the audio device. Failing that, the session runs silent.
    pub fn new(opts: &PlayOptions) -> Self {
        let audio = if opts.audio {
            let options = AudioOptions {
                buffer: Duration::from_millis(opts.audio_buffer_ms as u64),
            };
            match AudioPlayer::new(options) {
                Ok(a) => {
                    tracing::info!("playing audio on {}", a.device_name());
                    Some(a)
                }
                Err(e) => {
                    eprintln!("No sound: {e}");
                    None
                }
            }
        } else {
            None
        };
        let (feedback_tx, feedback_rx) = crossbeam_channel::unbounded();
        Self {
            audio,
            feedback_tx,
            feedback_rx,
            gamepads: None,
            mic: None,
            start_mic: opts.mic,
        }
    }

    /// The session's sinks: `video` plus audio and haptics.
    pub fn sinks(&self, video: VideoSink) -> Sinks {
        let tx = self.feedback_tx.clone();
        let mut rumble = HapticsRumble::new();
        Sinks {
            video,
            audio: self.audio.as_ref().map(AudioPlayer::sink),
            haptics: Some(Box::new(move |samples: &[i16]| {
                if let Some((low, high)) = rumble.push(samples) {
                    let _ = tx.send(Feedback::Rumble { low, high });
                }
            })),
        }
    }

    /// Starts controller input and the microphone control for `session`,
    /// and taps its event channel: the returned receiver gets every event
    /// after this has reacted to it.
    pub fn attach(
        &mut self,
        session: Arc<Session>,
        events: Receiver<SessionEvent>,
    ) -> Receiver<SessionEvent> {
        let mic = Arc::new(Mutex::new(Mic::new(session.clone())));
        self.mic = Some(mic.clone());
        let feedback = self.feedback_rx.clone();
        match Gamepads::start(session, feedback, move || mic.lock().unwrap().toggle()) {
            Ok(g) => self.gamepads = Some(g),
            Err(e) => eprintln!("No controller input: {e:#}"),
        }

        let handler = self.events();
        let (tx, rx) = crossbeam_channel::unbounded();
        let tap = std::thread::Builder::new()
            .name("play-events".into())
            .spawn(move || {
                for ev in events {
                    handler.handle(&ev);
                    if tx.send(ev).is_err() {
                        break;
                    }
                }
            });
        if let Err(e) = tap {
            tracing::error!("can't start the event thread: {e}");
        }
        rx
    }

    fn events(&self) -> PlayEvents {
        PlayEvents {
            feedback: self.feedback_tx.clone(),
            mic: self.mic.as_ref().map(Arc::downgrade),
            start_mic: self.start_mic,
        }
    }

    /// Stops input, microphone and audio, releasing every handle on the
    /// session; call it before dropping the last one.
    pub fn stop(mut self) {
        if let Some(mic) = self.mic.take() {
            mic.lock().unwrap().stop();
        }
        // Resets rumble and triggers, then shuts SDL down.
        self.gamepads = None;
        if let Some(a) = &self.audio {
            let stats = a.stats();
            tracing::info!(
                "audio: {} underruns, {} frames dropped",
                stats.underruns,
                stats.dropped_frames
            );
        }
    }
}

/// Session-event handling on the tap thread. Holds the microphone weakly so
/// it never keeps the session alive.
struct PlayEvents {
    feedback: Sender<Feedback>,
    mic: Option<Weak<Mutex<Mic>>>,
    start_mic: bool,
}

impl PlayEvents {
    /// Reacts to a session event (feedback for the controllers, starting
    /// the microphone once connected).
    fn handle(&self, ev: &SessionEvent) {
        if let Some(f) = Feedback::from_event(ev) {
            let _ = self.feedback.send(f);
        }
        match ev {
            SessionEvent::Connected if self.start_mic => {
                if let Some(mic) = self.mic.as_ref().and_then(Weak::upgrade) {
                    mic.lock().unwrap().toggle();
                }
            }
            SessionEvent::Quit { .. } => {
                if let Some(mic) = self.mic.as_ref().and_then(Weak::upgrade) {
                    mic.lock().unwrap().stop();
                }
            }
            _ => {}
        }
    }
}

/// Microphone on/off, mirrored on the console.
struct Mic {
    session: Arc<Session>,
    connected: bool,
    capture: Option<Microphone>,
}

impl Mic {
    fn new(session: Arc<Session>) -> Self {
        Self {
            session,
            connected: false,
            capture: None,
        }
    }

    fn toggle(&mut self) {
        if self.capture.is_some() {
            self.capture = None;
            let _ = self.session.toggle_microphone(true);
            eprintln!("Microphone off.");
            return;
        }
        let session = self.session.clone();
        let capture = match Microphone::start(move |pkt| {
            session.send_microphone(pkt);
        }) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("Microphone unavailable: {e}");
                return;
            }
        };
        if !self.connected {
            if let Err(e) = self.session.connect_microphone() {
                eprintln!("The console didn't accept the microphone: {e}");
                return;
            }
            self.connected = true;
        }
        let _ = self.session.toggle_microphone(false);
        eprintln!("Microphone on ({}).", capture.device_name());
        self.capture = Some(capture);
    }

    fn stop(&mut self) {
        self.capture = None;
    }
}
