//! A Remote Play streaming session.
//!
//! Wraps chiaki-ng's `ChiakiSession`: it connects to a paired console and
//! hands over video as encoded H.264/H.265 frames (one access unit each, Annex
//! B) through a callback, plus session events on a channel.

use std::ffi::{CString, c_void};
use std::sync::Mutex;

use chiaki_sys as sys;
use crossbeam_channel::{Receiver, Sender};

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
}

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
}

/// A running session. Stops and cleans up when dropped.
pub struct Session {
    session: Box<sys::ChiakiSession>,
    // Must outlive chiaki's threads; see Drop.
    callbacks: Box<Callbacks>,
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
            video: Mutex::new(video),
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

        // SAFETY: initialised above.
        if let Err(e) = check(unsafe { sys::chiaki_session_start(&mut *session) }) {
            // SAFETY: init succeeded, start did not; fini releases init's resources.
            unsafe { sys::chiaki_session_fini(&mut *session) };
            return Err(e);
        }
        Ok((
            Self {
                session,
                callbacks,
                joined: false,
            },
            rx,
        ))
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
        // SAFETY: joined, so no chiaki thread uses the session or callbacks.
        unsafe { sys::chiaki_session_fini(&mut *self.session) };
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets() {
        let p = VideoProfile::preset(Resolution::P720, 60, Codec::H264);
        assert_eq!((p.width, p.height, p.max_fps), (1280, 720, 60));
        assert!(p.bitrate > 0);
        let p = VideoProfile::preset(Resolution::P1080, 30, Codec::H265);
        assert_eq!((p.width, p.height, p.max_fps), (1920, 1080, 30));
    }
}
