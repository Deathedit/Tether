//! `tether stream`: wake the console, start a session and show its video.

use std::io::Write;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use rp_core::{Codec, Resolution, Session, SessionConfig, SessionEvent, VideoProfile, VideoSample};
use tether_core::{Store, WakeProgress};

use crate::video::{self, EncodedFrame, Shared};
use crate::window::{UserEvent, VideoWindow};

pub struct StreamOptions {
    pub resolution: Resolution,
    pub fps: u32,
    pub codec: Codec,
}

pub fn run(query: &str, opts: StreamOptions) -> Result<()> {
    let store = Store::open_default()?;
    let (paired, secrets) = crate::paired_with_secrets(&store, query)?;
    let account_id = store
        .account_id()?
        .context("no PSN account ID: run `tether login` first")?;

    // Wake it first if needed.
    let mut dots = false;
    let console = tether_core::ensure_awake(&paired, &secrets, Duration::from_secs(60), |p| {
        match p {
            WakeProgress::Waking if !dots => eprint!("Waking {}", paired.name),
            _ => eprint!("."),
        }
        dots = true;
        std::io::stderr().flush().ok();
    })?;
    if dots {
        eprintln!(" ready.");
    }

    let video_profile = VideoProfile::preset(opts.resolution, opts.fps, opts.codec);
    let config = SessionConfig {
        host: console.addr.to_string(),
        target: paired.target(),
        regist_key: secrets.regist_key,
        rp_key: secrets.rp_key,
        account_id,
        video: video_profile,
    };

    let window = VideoWindow::new()?;
    let proxy = window.proxy();
    let shared = Arc::new(Shared::default());

    // Encoded frames: chiaki's network thread -> decoder thread.
    let (frame_tx, frame_rx) = crossbeam_channel::unbounded::<EncodedFrame>();
    // Decoder asks for a keyframe -> event thread (which owns the session).
    let (idr_tx, idr_rx) = crossbeam_channel::bounded::<()>(1);

    let decoder = video::spawn_decoder(
        opts.codec,
        frame_rx,
        shared.clone(),
        {
            let proxy = proxy.clone();
            move || {
                let _ = proxy.send_event(UserEvent::Frame);
            }
        },
        move || {
            let _ = idr_tx.try_send(());
        },
    )?;

    eprintln!(
        "Connecting to {} ({}x{} @ {} fps, {:?})...",
        paired.name,
        video_profile.width,
        video_profile.height,
        video_profile.max_fps,
        video_profile.codec
    );
    let sink: rp_core::VideoSink = Box::new(move |sample: VideoSample<'_>| {
        frame_tx
            .send(EncodedFrame {
                data: sample.data.to_vec(),
                frames_lost: sample.frames_lost,
            })
            .is_ok()
    });
    let (session, events) = Session::start(&config, sink)?;
    let session = Arc::new(session);

    // Session events: login PIN prompt, quit -> close the window.
    let event_thread = {
        let session = session.clone();
        let proxy = proxy.clone();
        std::thread::Builder::new()
            .name("session-events".into())
            .spawn(move || -> Option<(String, bool)> {
                let mut quit = None;
                loop {
                    crossbeam_channel::select! {
                        recv(events) -> ev => match ev {
                            Ok(SessionEvent::Connected) => eprintln!("Connected."),
                            Ok(SessionEvent::LoginPinRequest { previous_incorrect }) => {
                                if previous_incorrect {
                                    eprintln!("That passcode was wrong.");
                                }
                                match crate::prompt("Remote Play passcode (set on the console): ") {
                                    Ok(pin) => { let _ = session.set_login_pin(&pin); }
                                    Err(_) => session.stop(),
                                }
                            }
                            Ok(SessionEvent::Quit { reason, error }) => {
                                quit = Some((reason, error));
                                let _ = proxy.send_event(UserEvent::SessionEnded);
                                break;
                            }
                            Ok(other) => tracing::debug!("session event: {other:?}"),
                            Err(_) => break,
                        },
                        recv(idr_rx) -> _ => { let _ = session.request_idr(); }
                    }
                }
                quit
            })?
    };

    let title = format!("Tether - {}", paired.name);
    let close_session = session.clone();
    window.run(title, shared, move || close_session.stop())?;

    // Window closed (by the user or because the session ended).
    session.stop();
    let quit = event_thread.join().ok().flatten();
    // Dropping the last session handle joins chiaki and closes the frame
    // channel, which ends the decoder thread.
    drop(session);
    let stats = decoder.join().ok();

    if let Some(stats) = stats {
        tracing::info!(
            "decoded {} frames, {} decoder errors",
            stats.decoded,
            stats.errors
        );
    }
    match quit {
        Some((reason, true)) => bail!("session ended: {reason}"),
        Some((reason, false)) => eprintln!("Session ended: {reason}"),
        None => eprintln!("Session ended."),
    }
    Ok(())
}
