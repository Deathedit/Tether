//! A Remote Play streaming session.
//!
//! Wraps chiaki-ng's `ChiakiSession`: it connects to a paired console and
//! hands over video as encoded H.264/H.265 frames (one access unit each, Annex
//! B) through a callback, plus session events on a channel. Audio (Opus) and
//! DualSense haptics (PCM) come through optional callbacks too; controller
//! input and microphone audio go the other way.

use std::ffi::{CString, c_void};
use std::sync::Mutex;

use chiaki_sys as sys;
use crossbeam_channel::{Receiver, Sender};

use crate::controller::ControllerState;
use crate::error::{Error, Result, check};
use crate::ffi;
use crate::psn::AccountId;
use crate::target::Target;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Codec {
    H264,
    H265,
    H265Hdr,
}

impl Codec {
    #[allow(clippy::unnecessary_cast)] // C enums are i32 on MSVC, u32 elsewhere
    fn to_sys(self) -> sys::ChiakiCodec {
        (match self {
            Codec::H264 => sys::CHIAKI_CODEC_H264,
            Codec::H265 => sys::CHIAKI_CODEC_H265,
            Codec::H265Hdr => sys::CHIAKI_CODEC_H265_HDR,
        }) as sys::ChiakiCodec
    }

    pub fn is_h265(self) -> bool {
        !matches!(self, Codec::H264)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resolution {
    P360,
    P540,
    P720,
    P1080,
}

/// What the console should stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VideoProfile {
    pub width: u32,
    pub height: u32,
    pub max_fps: u32,
    /// kbit/s
    pub bitrate: u32,
    pub codec: Codec,
}

impl VideoProfile {
    /// chiaki's presets (resolution and matching bitrate). `fps` is 30 or 60.
    pub fn preset(resolution: Resolution, fps: u32, codec: Codec) -> Self {
        let res = match resolution {
            Resolution::P360 => sys::CHIAKI_VIDEO_RESOLUTION_PRESET_360p,
            Resolution::P540 => sys::CHIAKI_VIDEO_RESOLUTION_PRESET_540p,
            Resolution::P720 => sys::CHIAKI_VIDEO_RESOLUTION_PRESET_720p,
            Resolution::P1080 => sys::CHIAKI_VIDEO_RESOLUTION_PRESET_1080p,
        };
        let fps_preset = if fps >= 60 {
            sys::CHIAKI_VIDEO_FPS_PRESET_60
        } else {
            sys::CHIAKI_VIDEO_FPS_PRESET_30
        };
        let mut raw = sys::ChiakiConnectVideoProfile::default();
        // SAFETY: fills a plain struct.
        unsafe { sys::chiaki_connect_video_profile_preset(&mut raw, res, fps_preset) };
        VideoProfile {
            width: raw.width,
            height: raw.height,
            max_fps: raw.max_fps,
            bitrate: raw.bitrate,
            codec,
        }
    }

    fn to_sys(self) -> sys::ChiakiConnectVideoProfile {
        sys::ChiakiConnectVideoProfile {
            width: self.width,
            height: self.height,
            max_fps: self.max_fps,
            bitrate: self.bitrate,
            codec: self.codec.to_sys(),
        }
    }
}

/// Everything needed to connect to a paired console.
#[derive(Clone)]
pub struct SessionConfig {
    pub host: String,
    pub target: Target,
    /// `RegisteredConsole::regist_key` from pairing.
    pub regist_key: [u8; 16],
    /// `RegisteredConsole::rp_key` from pairing (chiaki calls it "morning").
    pub rp_key: [u8; 16],
    pub account_id: AccountId,
    pub video: VideoProfile,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionEvent {
    /// The stream is up; video frames follow.
    Connected,
    /// The console asks for its Remote Play login passcode.
    LoginPinRequest {
        previous_incorrect: bool,
    },
    /// The console's name, as it reports it during the session.
    Nickname(String),
    Rumble {
        left: u8,
        right: u8,
    },
    /// The session ended. `error` is false for a normal stop.
    Quit {
        reason: String,
        error: bool,
    },
    /// DualSense adaptive-trigger effects: an effect type and its 10
    /// parameter bytes per trigger, in the controller's own format.
    TriggerEffects {
        type_left: u8,
        left: [u8; 10],
        type_right: u8,
        right: [u8; 10],
    },
    /// Light bar colour (red, green, blue).
    LedColor([u8; 3]),
    /// Player number for the controller's player lights.
    PlayerIndex(u8),
    /// Treat the controller's current position as level.
    MotionReset,
    /// The console's vibration strength setting.
    HapticIntensity(EffectIntensity),
    /// The console's adaptive-trigger strength setting.
    TriggerIntensity(EffectIntensity),
}

/// Strength setting for vibration or trigger effects, from the console's
/// accessibility settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffectIntensity {
    Off,
    Weak,
    Medium,
    Strong,
}

/// The console's audio stream format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioFormat {
    pub channels: u8,
    /// Bits per sample of the decoded PCM.
    pub bits: u8,
    /// Hz
    pub rate: u32,
    /// Samples per channel in one frame.
    pub frame_size: u32,
}

/// What the audio callback receives.
#[derive(Debug)]
pub enum AudioData<'a> {
    /// The stream format; sent before the first frame and again if it changes.
    Format(AudioFormat),
    /// One Opus packet (one frame).
    Frame(&'a [u8]),
    /// A frame was lost: conceal it (Opus packet-loss concealment).
    Lost,
}

/// Receives audio, in order, on chiaki's network thread. Keep it fast.
pub type AudioSink = Box<dyn FnMut(AudioData<'_>) + Send>;

/// Receives DualSense haptics: 16-bit PCM, interleaved stereo (left and
/// right actuator) at 3 kHz, 10 ms per call. On chiaki's network thread.
pub type HapticsSink = Box<dyn FnMut(&[i16]) + Send>;

/// Where the session delivers media.
pub struct Sinks {
    pub video: VideoSink,
    pub audio: Option<AudioSink>,
    pub haptics: Option<HapticsSink>,
}

impl Sinks {
    pub fn video_only(video: VideoSink) -> Self {
        Self {
            video,
            audio: None,
            haptics: None,
        }
    }
}

/// Microphone packets chiaki can send: exactly this many bytes of Opus.
pub const MIC_PACKET_SIZE: usize = 40;
/// Microphone format the console expects: 48 kHz stereo, 10 ms frames.
pub const MIC_FORMAT: AudioFormat = AudioFormat {
    channels: 2,
    bits: 16,
    rate: 48_000,
    frame_size: 480,
};

/// One encoded video frame from the console.
pub struct VideoSample<'a> {
    /// One access unit, Annex B.
    pub data: &'a [u8],
    /// Frames lost since the previous sample.
    pub frames_lost: i32,
    /// This frame was rebuilt from error correction; it may contain glitches.
    pub recovered: bool,
}

/// Receives every video frame, on chiaki's network thread: keep it fast and
/// hand frames to a decoder thread. Return `false` if the frame could not be
/// used (chiaki then asks the console for a new keyframe).
pub type VideoSink = Box<dyn FnMut(VideoSample<'_>) -> bool + Send>;

struct Callbacks {
    events: Sender<SessionEvent>,
    video: Mutex<VideoSink>,
    audio: Option<Mutex<AudioSink>>,
    haptics: Option<Mutex<HapticsSink>>,
}

/// A running session. Stops and cleans up when dropped.
pub struct Session {
    session: Box<sys::ChiakiSession>,
    // Must outlive chiaki's threads; see Drop.
    callbacks: Box<Callbacks>,
    /// Sends microphone packets; points into `session`.
    mic: Box<sys::ChiakiAudioSender>,
    joined: bool,
}

// SAFETY: chiaki's session API is thread-safe (it locks internally); this
// handle is only used to send requests and to stop it.
unsafe impl Send for Session {}
unsafe impl Sync for Session {}

impl Session {
    /// Connects to the console. Video goes to `video`, events to the
    /// returned channel.
    pub fn start(
        config: &SessionConfig,
        video: VideoSink,
    ) -> Result<(Self, Receiver<SessionEvent>)> {
        Self::start_with(config, Sinks::video_only(video))
    }

    /// Like [`Session::start`], with audio and haptics callbacks as well.
    pub fn start_with(
        config: &SessionConfig,
        sinks: Sinks,
    ) -> Result<(Self, Receiver<SessionEvent>)> {
        let host = CString::new(config.host.as_str())
            .map_err(|_| Error::InvalidHost(config.host.clone()))?;
        let log = ffi::log();

        let mut regist_key = [0 as std::ffi::c_char; 16];
        for (dst, &src) in regist_key.iter_mut().zip(config.regist_key.iter()) {
            *dst = src as std::ffi::c_char;
        }
        let mut info = sys::ChiakiConnectInfo {
            ps5: config.target.is_ps5(),
            host: host.as_ptr(),
            regist_key,
            morning: config.rp_key,
            video_profile: config.video.to_sys(),
            video_profile_auto_downgrade: true,
            enable_keyboard: false,
            enable_dualsense: true,
            psn_account_id: *config.account_id.as_bytes(),
            // chiaki-ng's defaults.
            packet_loss_max: 0.05,
            enable_idr_on_fec_failure: true,
            ..Default::default()
        };

        let (tx, rx) = crossbeam_channel::unbounded();
        let callbacks = Box::new(Callbacks {
            events: tx,
            video: Mutex::new(sinks.video),
            audio: sinks.audio.map(Mutex::new),
            haptics: sinks.haptics.map(Mutex::new),
        });

        let mut session = Box::new(sys::ChiakiSession::default());
        // SAFETY: `session` is boxed (stable address for chiaki's threads);
        // chiaki copies everything it needs out of `info` during init.
        check(unsafe { sys::chiaki_session_init(&mut *session, &mut info, log) })?;

        // chiaki_session_set_*_cb are static inline in C, so set the fields.
        let user = &*callbacks as *const Callbacks as *mut c_void;
        session.event_cb = Some(event_cb);
        session.event_cb_user = user;
        session.video_sample_cb = Some(video_cb);
        session.video_sample_cb_user = user;
        // Likewise chiaki_session_set_audio_sink / _set_haptics_sink. chiaki
        // only calls these while the stream runs, after start.
        if callbacks.audio.is_some() {
            session.audio_sink = sys::ChiakiAudioSink {
                user,
                header_cb: Some(audio_header_cb),
                frame_cb: Some(audio_frame_cb),
            };
        }
        if callbacks.haptics.is_some() {
            session.haptics_sink = sys::ChiakiAudioSink {
                user,
                header_cb: None,
                frame_cb: Some(haptics_frame_cb),
            };
        }

        // The microphone sender only keeps a pointer to the session's stream
        // connection; it sends nothing until `send_microphone`.
        let mut mic = Box::new(sys::ChiakiAudioSender::default());
        // SAFETY: both boxed (stable addresses); the session is initialised.
        let mic_err = unsafe { sys::chiaki_audio_sender_init(&mut *mic, log, &mut *session) };
        if let Err(e) = check(mic_err) {
            // SAFETY: init succeeded; nothing started yet.
            unsafe { sys::chiaki_session_fini(&mut *session) };
            return Err(e);
        }

        // SAFETY: initialised above.
        if let Err(e) = check(unsafe { sys::chiaki_session_start(&mut *session) }) {
            // SAFETY: init succeeded, start did not; fini releases init's resources.
            unsafe {
                sys::chiaki_audio_sender_fini(&mut *mic);
                sys::chiaki_session_fini(&mut *session);
            }
            return Err(e);
        }
        Ok((
            Self {
                session,
                callbacks,
                mic,
                joined: false,
            },
            rx,
        ))
    }

    /// Sends the controller state to the console. Call it whenever the
    /// input changes; chiaki sends it right away (and repeats it as needed).
    pub fn set_controller_state(&self, state: &ControllerState) -> Result<()> {
        let mut raw = state.to_sys();
        // SAFETY: valid session; chiaki copies the state under its own lock.
        check(unsafe { sys::chiaki_session_set_controller_state(self.raw(), &mut raw) })
    }

    /// Tells the console a microphone is attached. Call once, after
    /// `Connected`, before the first [`Session::toggle_microphone`].
    pub fn connect_microphone(&self) -> Result<()> {
        // SAFETY: valid session.
        check(unsafe { sys::chiaki_session_connect_microphone(self.raw()) })
    }

    /// Mutes or unmutes the microphone on the console's side.
    pub fn toggle_microphone(&self, muted: bool) -> Result<()> {
        // SAFETY: valid session.
        check(unsafe { sys::chiaki_session_toggle_microphone(self.raw(), muted) })
    }

    /// Sends one 10 ms microphone frame: Opus at [`MIC_FORMAT`], exactly
    /// [`MIC_PACKET_SIZE`] bytes (other sizes are dropped). Does nothing
    /// unless the stream is up. Returns whether it was sent.
    pub fn send_microphone(&self, opus: &[u8]) -> bool {
        if opus.len() != MIC_PACKET_SIZE {
            return false;
        }
        let session = self.raw();
        // The stream's socket and keys only exist while the feedback sender
        // is active; chiaki flips that flag under this mutex, which lives as
        // long as the session.
        // SAFETY: valid session; the mutex is initialised in session init.
        unsafe {
            let mutex = &raw mut (*session).stream_connection.feedback_sender_mutex;
            if sys::chiaki_mutex_lock(mutex) != sys::CHIAKI_ERR_SUCCESS {
                return false;
            }
            let active = (*session).stream_connection.feedback_sender_active;
            if active {
                let mic =
                    &*self.mic as *const sys::ChiakiAudioSender as *mut sys::ChiakiAudioSender;
                // chiaki doesn't write through the data pointer.
                sys::chiaki_audio_sender_opus_data(mic, opus.as_ptr() as *mut u8, opus.len());
            }
            sys::chiaki_mutex_unlock(mutex);
            active
        }
    }

    fn raw(&self) -> *mut sys::ChiakiSession {
        &*self.session as *const sys::ChiakiSession as *mut sys::ChiakiSession
    }

    /// Answers a `LoginPinRequest` with the console's Remote Play passcode.
    pub fn set_login_pin(&self, pin: &str) -> Result<()> {
        // SAFETY: valid session; chiaki copies the bytes.
        check(unsafe { sys::chiaki_session_set_login_pin(self.raw(), pin.as_ptr(), pin.len()) })
    }

    /// Asks the console for a keyframe (e.g. after decoder errors).
    pub fn request_idr(&self) -> Result<()> {
        // SAFETY: valid session.
        check(unsafe { sys::chiaki_session_request_idr(self.raw()) })
    }

    /// Ends the session. A `Quit` event follows.
    pub fn stop(&self) {
        // SAFETY: valid session; stop only signals chiaki's threads.
        let _ = unsafe { sys::chiaki_session_stop(self.raw()) };
    }

    /// Puts the console into rest mode and ends the session.
    pub fn goto_bed(&self) -> Result<()> {
        // SAFETY: valid session.
        check(unsafe { sys::chiaki_session_goto_bed(self.raw()) })
    }

    /// Waits for chiaki's session thread to finish (after `stop` or a quit).
    pub fn join(mut self) {
        self.join_inner();
    }

    fn join_inner(&mut self) {
        if !self.joined {
            // SAFETY: valid, started session.
            unsafe { sys::chiaki_session_join(&mut *self.session) };
            self.joined = true;
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.stop();
        self.join_inner();
        // SAFETY: joined, so no chiaki thread uses the session or callbacks,
        // and `&mut self` means no `send_microphone` is running.
        unsafe {
            sys::chiaki_audio_sender_fini(&mut *self.mic);
            sys::chiaki_session_fini(&mut *self.session);
        }
        let _ = &self.callbacks;
    }
}

unsafe extern "C" fn event_cb(event: *mut sys::ChiakiEvent, user: *mut c_void) {
    if event.is_null() || user.is_null() {
        return;
    }
    // SAFETY: `user` is the boxed Callbacks; `event` is valid for this call.
    let (cb, event) = unsafe { (&*(user as *const Callbacks), &*event) };
    // SAFETY (union reads below): each arm reads the member that matches `type_`.
    let ev = unsafe {
        let u = &event.__bindgen_anon_1;
        match event.type_ {
            sys::CHIAKI_EVENT_CONNECTED => SessionEvent::Connected,
            sys::CHIAKI_EVENT_LOGIN_PIN_REQUEST => SessionEvent::LoginPinRequest {
                previous_incorrect: u.login_pin_request.pin_incorrect,
            },
            sys::CHIAKI_EVENT_NICKNAME_RECEIVED => {
                let bytes: Vec<u8> = u
                    .server_nickname
                    .iter()
                    .map(|&c| c as u8)
                    .take_while(|&b| b != 0)
                    .collect();
                SessionEvent::Nickname(String::from_utf8_lossy(&bytes).into_owned())
            }
            sys::CHIAKI_EVENT_RUMBLE => SessionEvent::Rumble {
                left: u.rumble.left,
                right: u.rumble.right,
            },
            sys::CHIAKI_EVENT_QUIT => {
                let reason = u.quit.reason;
                let mut text =
                    ffi::opt_string(sys::chiaki_quit_reason_string(reason)).unwrap_or_default();
                if let Some(detail) = ffi::opt_string(u.quit.reason_str) {
                    text = format!("{text}: {detail}");
                }
                let error = reason != sys::CHIAKI_QUIT_REASON_STOPPED
                    && reason != sys::CHIAKI_QUIT_REASON_STREAM_CONNECTION_REMOTE_SHUTDOWN;
                SessionEvent::Quit {
                    reason: text,
                    error,
                }
            }
            sys::CHIAKI_EVENT_TRIGGER_EFFECTS => {
                let t = &u.trigger_effects;
                SessionEvent::TriggerEffects {
                    type_left: t.type_left,
                    left: t.left,
                    type_right: t.type_right,
                    right: t.right,
                }
            }
            sys::CHIAKI_EVENT_LED_COLOR => SessionEvent::LedColor(u.led_state),
            sys::CHIAKI_EVENT_PLAYER_INDEX => SessionEvent::PlayerIndex(u.player_index),
            sys::CHIAKI_EVENT_MOTION_RESET => SessionEvent::MotionReset,
            sys::CHIAKI_EVENT_HAPTIC_INTENSITY => {
                SessionEvent::HapticIntensity(intensity(u.intensity))
            }
            sys::CHIAKI_EVENT_TRIGGER_INTENSITY => {
                SessionEvent::TriggerIntensity(intensity(u.intensity))
            }
            _ => return,
        }
    };
    let _ = cb.events.send(ev);
}

unsafe extern "C" fn video_cb(
    buf: *mut u8,
    buf_size: usize,
    frames_lost: i32,
    frame_recovered: bool,
    user: *mut c_void,
) -> bool {
    if buf.is_null() || user.is_null() {
        return false;
    }
    // SAFETY: `user` is the boxed Callbacks; `buf` holds `buf_size` bytes
    // for this call.
    let (cb, data) = unsafe {
        (
            &*(user as *const Callbacks),
            std::slice::from_raw_parts(buf, buf_size),
        )
    };
    let mut sink = cb.video.lock().unwrap();
    sink(VideoSample {
        data,
        frames_lost,
        recovered: frame_recovered,
    })
}

#[allow(clippy::unnecessary_cast)] // C enums are i32 on MSVC, u32 elsewhere
fn intensity(raw: sys::ChiakiDualSenseEffectIntensity) -> EffectIntensity {
    // chiaki's values: Off = 0, Strong = 1, Medium = 2, Weak = 3.
    match raw as u32 {
        1 => EffectIntensity::Strong,
        2 => EffectIntensity::Medium,
        3 => EffectIntensity::Weak,
        _ => EffectIntensity::Off,
    }
}

unsafe extern "C" fn audio_header_cb(header: *mut sys::ChiakiAudioHeader, user: *mut c_void) {
    if header.is_null() || user.is_null() {
        return;
    }
    // SAFETY: `user` is the boxed Callbacks; `header` is valid for this call.
    let (cb, h) = unsafe { (&*(user as *const Callbacks), &*header) };
    if let Some(audio) = &cb.audio {
        let format = AudioFormat {
            channels: h.channels,
            bits: h.bits,
            rate: h.rate,
            frame_size: h.frame_size,
        };
        (audio.lock().unwrap())(AudioData::Format(format));
    }
}

unsafe extern "C" fn audio_frame_cb(buf: *mut u8, buf_size: usize, user: *mut c_void) {
    if user.is_null() {
        return;
    }
    // SAFETY: `user` is the boxed Callbacks.
    let cb = unsafe { &*(user as *const Callbacks) };
    let Some(audio) = &cb.audio else { return };
    // chiaki passes NULL for a frame it gave up waiting for.
    let data = if buf.is_null() || buf_size == 0 {
        AudioData::Lost
    } else {
        // SAFETY: `buf` holds `buf_size` bytes for this call.
        AudioData::Frame(unsafe { std::slice::from_raw_parts(buf, buf_size) })
    };
    (audio.lock().unwrap())(data);
}

unsafe extern "C" fn haptics_frame_cb(buf: *mut u8, buf_size: usize, user: *mut c_void) {
    if buf.is_null() || user.is_null() {
        return;
    }
    // SAFETY: `user` is the boxed Callbacks; `buf` holds `buf_size` bytes.
    let (cb, bytes) = unsafe {
        (
            &*(user as *const Callbacks),
            std::slice::from_raw_parts(buf, buf_size),
        )
    };
    let Some(haptics) = &cb.haptics else { return };
    // Little-endian i16, possibly unaligned. Frames are 120 bytes.
    let mut samples = [0i16; 256];
    let n = (bytes.len() / 2).min(samples.len());
    for (s, b) in samples.iter_mut().zip(bytes.chunks_exact(2)) {
        *s = i16::from_le_bytes([b[0], b[1]]);
    }
    (haptics.lock().unwrap())(&samples[..n]);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intensities() {
        assert_eq!(intensity(0 as _), EffectIntensity::Off);
        assert_eq!(intensity(1 as _), EffectIntensity::Strong);
        assert_eq!(intensity(3 as _), EffectIntensity::Weak);
    }

    #[test]
    fn presets() {
        let p = VideoProfile::preset(Resolution::P720, 60, Codec::H264);
        assert_eq!((p.width, p.height, p.max_fps), (1280, 720, 60));
        assert!(p.bitrate > 0);
        let p = VideoProfile::preset(Resolution::P1080, 30, Codec::H265);
        assert_eq!((p.width, p.height, p.max_fps), (1920, 1080, 30));
    }
}
