//! The Slint desktop UI: wires the screens in `ui/` to discovery, pairing,
//! sessions and settings.
//!
//! Everything slow (discovery, sign-in, pairing, waking, the session, video)
//! runs on worker threads, which hand results back to the UI thread with
//! [`post`]. The UI thread owns the [`App`] state.

mod model;

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use rp_core::{
    Console, ConsoleState as RpState, DiscoveryOptions, RegistRequest, RegisteredConsole,
};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};
use tether_core::{
    DecoderPreference, Store, StreamSettings, VideoCodec, VideoResolution,
    settings::BITRATE_RANGE_KBPS,
};

use crate::play::PlayOptions;
use crate::stream::{EventSink, Gpu, SharedGpu, Stream, StreamEvent, StreamRequest};
use crate::video::Backend;
use model::{ConnectProgress, RowState, StatsMeter};

slint::include_modules!();

/// Where the pairing code is on the console.
const PAIR_SCREEN: &str = "Settings › System › Remote Play › Link Device";

/// Per-run stream settings from the command line; not saved.
#[derive(Debug, Default, Clone, Copy)]
pub struct Overrides {
    pub resolution: Option<VideoResolution>,
    pub fps: Option<u32>,
    pub codec: Option<VideoCodec>,
    /// Don't play the console's sound.
    pub no_audio: bool,
    /// Audio jitter buffer; default 30 ms.
    pub audio_buffer_ms: Option<u32>,
    /// Start with the microphone on.
    pub mic: bool,
}

impl Overrides {
    /// Sound, microphone and controller options for a session.
    fn play_options(self) -> PlayOptions {
        PlayOptions {
            audio: !self.no_audio,
            audio_buffer_ms: self.audio_buffer_ms.unwrap_or(30),
            mic: self.mic,
        }
    }

    fn apply(self, mut s: StreamSettings) -> StreamSettings {
        if let Some(r) = self.resolution {
            s.resolution = r;
            s.bitrate_kbps = None;
        }
        if let Some(f) = self.fps {
            s.fps = f;
        }
        if let Some(c) = self.codec {
            s.codec = c;
        }
        s
    }
}

#[derive(Debug, Default)]
pub struct Options {
    /// Connect to this paired console right away (`tether stream`), and quit
    /// when the session ends.
    pub stream: Option<String>,
    pub overrides: Overrides,
}

pub fn run(opts: Options) -> Result<()> {
    slint::BackendSelector::new()
        .require_wgpu_30(slint::wgpu_30::WGPUConfiguration::default())
        .select()
        .context("could not set up the GPU renderer (wgpu: Vulkan or Direct3D 12)")?;

    let store = Store::open_default()?;
    let auto_connect = match &opts.stream {
        Some(q) => Some(
            store
                .find(q)
                .with_context(|| {
                    format!("{q:?} is not paired. Pair it with `tether pair <address>`.")
                })?
                .host_id
                .clone(),
        ),
        None => None,
    };

    let window = AppWindow::new()?;
    window.global::<Theme>().set_mono(
        if cfg!(windows) {
            "Consolas"
        } else {
            "monospace"
        }
        .into(),
    );

    let gpu: SharedGpu = Default::default();
    {
        let gpu = gpu.clone();
        window
            .window()
            .set_rendering_notifier(move |state, api| {
                if let (
                    slint::RenderingState::RenderingSetup,
                    slint::GraphicsAPI::WGPU30 { device, queue, .. },
                ) = (state, api)
                {
                    let _ = gpu.set(Gpu {
                        device: device.clone(),
                        queue: queue.clone(),
                    });
                }
            })
            .context("Slint is not rendering with wgpu")?;
    }

    let app = Rc::new(App {
        window: window.as_weak(),
        store: RefCell::new(store),
        gpu,
        overrides: opts.overrides,
        exit_after_stream: auto_connect.is_some(),
        discovered: RefCell::new(Vec::new()),
        pairing: RefCell::new(PairingCtx::default()),
        stream: RefCell::new(None),
        stopping: RefCell::new(Vec::new()),
        next_stream_id: Cell::new(1),
        settings_return: Cell::new(Screen::Consoles),
        hw_available: Cell::new(None),
        last_backend: Cell::new(None),
        exit_error: RefCell::new(None),
    });
    APP.with(|a| *a.borrow_mut() = Some(app.clone()));

    app.wire_consoles();
    app.wire_pairing();
    app.wire_connecting();
    app.wire_session();
    app.wire_settings();
    app.refresh_consoles();
    app.refresh_settings();
    start_discovery();
    probe_hardware();

    let stats_timer = slint::Timer::default();
    stats_timer.start(
        slint::TimerMode::Repeated,
        Duration::from_millis(250),
        || {
            with_app(|app| app.update_stats());
        },
    );

    if let Some(host_id) = auto_connect {
        slint::Timer::single_shot(Duration::from_millis(100), move || {
            with_app(|app| app.connect(&host_id));
        });
    }

    window.run()?;

    // Let sessions close properly (and log their stats) before exiting.
    // Their `Ended` events can't be handled any more, so save the summary
    // here.
    let mut streams: Vec<Stream> = app
        .stopping
        .borrow_mut()
        .drain(..)
        .map(|(_, s)| s)
        .collect();
    streams.extend(app.stream.borrow_mut().take().map(|s| s.stream));
    for stream in streams {
        if let Some(summary) = stream.finish(Duration::from_secs(5))
            && let Err(e) = app.store.borrow_mut().set_last_session(summary)
        {
            tracing::warn!("could not save session summary: {e}");
        }
    }
    APP.with(|a| a.borrow_mut().take());
    if let Some(e) = app.exit_error.borrow_mut().take() {
        bail!("{e}");
    }
    Ok(())
}

thread_local! {
    static APP: RefCell<Option<Rc<App>>> = const { RefCell::new(None) };
}

fn with_app(f: impl FnOnce(&Rc<App>)) {
    let app = APP.with(|a| a.borrow().clone());
    if let Some(app) = app {
        f(&app);
    }
}

/// Runs `f` on the UI thread with the app state.
fn post(f: impl FnOnce(&Rc<App>) + Send + 'static) {
    let _ = slint::invoke_from_event_loop(move || with_app(f));
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[derive(Default)]
struct PairingCtx {
    /// The discovered console to link (unless an IP is typed in).
    target: Option<Console>,
    /// Host ID of the console just linked.
    linked: Option<String>,
    /// Opened from settings just to sign in.
    sign_in_only: bool,
    return_to: Option<Screen>,
}

struct ActiveStream {
    id: u64,
    stream: Stream,
    host_id: String,
    settings: StreamSettings,
    progress: ConnectProgress,
    meter: StatsMeter,
    frames_lost: u64,
}

struct App {
    window: slint::Weak<AppWindow>,
    store: RefCell<Store>,
    gpu: SharedGpu,
    overrides: Overrides,
    exit_after_stream: bool,
    discovered: RefCell<Vec<Console>>,
    pairing: RefCell<PairingCtx>,
    stream: RefCell<Option<ActiveStream>>,
    /// Streams told to stop whose session hasn't closed yet.
    stopping: RefCell<Vec<(u64, Stream)>>,
    next_stream_id: Cell<u64>,
    settings_return: Cell<Screen>,
    /// Whether the platform's hardware decoder could be opened.
    hw_available: Cell<Option<bool>>,
    /// Decoder used by the last stream.
    last_backend: Cell<Option<Backend>>,
    exit_error: RefCell<Option<String>>,
}

fn model<T: Clone + 'static>(items: Vec<T>) -> ModelRc<T> {
    ModelRc::new(VecModel::from(items))
}

fn strings(items: impl IntoIterator<Item = impl Into<SharedString>>) -> ModelRc<SharedString> {
    model(items.into_iter().map(Into::into).collect())
}

impl App {
    fn ui(&self) -> AppWindow {
        self.window.upgrade().expect("window outlives the app")
    }

    fn screen(&self) -> Screen {
        self.ui().get_screen()
    }

    fn show(&self, screen: Screen) {
        let ui = self.ui();
        ui.set_screen(screen);
        if screen == Screen::Session {
            ui.invoke_focus_session();
        }
    }

    fn banner(&self, text: impl Into<SharedString>) {
        self.ui().global::<ConsolesState>().set_banner(text.into());
    }

    fn settings(&self) -> StreamSettings {
        self.store.borrow().config().stream
    }

    // ------------------------------------------------------------ consoles

    fn wire_consoles(self: &Rc<Self>) {
        let ui = self.ui();
        let g = ui.global::<ConsolesState>();
        let app = self.clone();
        g.on_connect(move |id| app.connect(&id));
        let app = self.clone();
        g.on_link(move |id| app.open_pairing(Some(id.as_str()), false, Screen::Consoles));
        let app = self.clone();
        g.on_add_console(move || app.open_pairing(None, false, Screen::Consoles));
        let app = self.clone();
        g.on_open_settings(move || app.open_settings(0));
        let app = self.clone();
        g.on_open_account(move || app.open_settings(1));
    }

    fn decoder_label(&self) -> String {
        if self.settings().decoder == DecoderPreference::Software {
            return "Software".into();
        }
        if let Some(b) = self.last_backend.get() {
            return b.to_string();
        }
        match (Backend::platform_default(), self.hw_available.get()) {
            (Some(hw), Some(true)) => hw.to_string(),
            (Some(hw), Some(false)) => format!("Software (no {hw})"),
            (Some(hw), None) => format!("Auto · {hw}"),
            (None, _) => "Software".into(),
        }
    }

    fn refresh_consoles(&self) {
        let ui = self.ui();
        let g = ui.global::<ConsolesState>();
        let store = self.store.borrow();
        let rows = model::build_rows(store.paired(), &self.discovered.borrow());
        g.set_summary(model::summary(&rows).into());
        g.set_consoles(model(
            rows.iter()
                .map(|r| ConsoleItem {
                    host_id: r.host_id.clone().into(),
                    name: r.name.clone().into(),
                    meta: r.meta.clone().into(),
                    state: match r.state {
                        RowState::Ready => ConsoleState::Ready,
                        RowState::Rest => ConsoleState::Rest,
                        RowState::Away => ConsoleState::Away,
                        RowState::Unknown => ConsoleState::Unknown,
                    },
                    paired: r.paired,
                    featured: r.featured,
                })
                .collect(),
        ));
        let signed_in = matches!(store.account_id(), Ok(Some(_)));
        g.set_signed_in(signed_in);
        g.set_account_text(
            if signed_in {
                "Signed in"
            } else {
                "Not signed in"
            }
            .into(),
        );
        let s = self.overrides.apply(store.config().stream);
        g.set_start_text(format!("Starts at {}", model::stream_label(&s)).into());
        g.set_video_setting(format!("{}{} {}", s.resolution, s.fps(), s.codec.label()).into());
        match &store.config().last_session {
            Some(last) => {
                let (title, detail) = model::last_session_lines(last, unix_now());
                g.set_has_last_session(true);
                g.set_last_session_title(title.into());
                g.set_last_session_detail(detail.into());
            }
            None => g.set_has_last_session(false),
        }
        drop(store);
        g.set_decoder(self.decoder_label().into());
    }

    fn set_discovered(&self, list: Vec<Console>) {
        {
            let mut store = self.store.borrow_mut();
            for c in &list {
                if let Err(e) = store.update_address(&c.host_id, &c.addr.to_string()) {
                    tracing::warn!("could not save new address of {}: {e}", c.name);
                }
            }
        }
        *self.discovered.borrow_mut() = list;
        self.refresh_consoles();
        if self.screen() == Screen::Pairing {
            self.refresh_pairing_target();
        }
    }

    // ------------------------------------------------------------ pairing

    fn wire_pairing(self: &Rc<Self>) {
        let ui = self.ui();
        let g = ui.global::<PairingState>();
        g.on_open_sign_in(|| {
            let _ = open::that_detached(rp_core::psn::login_url());
        });
        let app = self.clone();
        g.on_submit_sign_in(move |text| app.submit_sign_in(&text));
        let app = self.clone();
        g.on_pin_edited(move |text| app.set_pin(&model::sanitize_pin(&text)));
        let app = self.clone();
        g.on_submit_pin(move || app.submit_pin());
        let app = self.clone();
        g.on_connect_now(move || {
            let linked = app.pairing.borrow().linked.clone();
            if let Some(id) = linked {
                app.connect(&id);
            }
        });
        let app = self.clone();
        g.on_back(move || app.close_pairing());
        let app = self.clone();
        g.on_close(move || app.close_pairing());
    }

    fn open_pairing(&self, target: Option<&str>, sign_in_only: bool, return_to: Screen) {
        let signed_in = matches!(self.store.borrow().account_id(), Ok(Some(_)));
        let target = target.and_then(|id| {
            self.discovered
                .borrow()
                .iter()
                .find(|c| c.host_id.eq_ignore_ascii_case(id))
                .cloned()
        });
        *self.pairing.borrow_mut() = PairingCtx {
            target,
            linked: None,
            sign_in_only,
            return_to: Some(return_to),
        };
        let ui = self.ui();
        let g = ui.global::<PairingState>();
        g.set_step(if sign_in_only || !signed_in { 1 } else { 2 });
        g.set_busy(false);
        g.set_error("".into());
        g.set_sign_in_text("".into());
        g.set_manual_ip(false);
        g.set_ip_text("".into());
        g.set_show_help(false);
        self.set_pin("");
        self.refresh_pairing_target();
        self.show(Screen::Pairing);
    }

    fn close_pairing(&self) {
        let to = self.pairing.borrow().return_to.unwrap_or(Screen::Consoles);
        self.show(to);
    }

    /// Picks a console to link if none is chosen yet, and shows it.
    fn refresh_pairing_target(&self) {
        let mut ctx = self.pairing.borrow_mut();
        let discovered = self.discovered.borrow();
        if let Some(t) = &ctx.target {
            // Keep its state current.
            if let Some(fresh) = discovered.iter().find(|c| c.host_id == t.host_id) {
                ctx.target = Some(fresh.clone());
            }
        } else {
            let store = self.store.borrow();
            let unpaired = |c: &&Console| store.find(&c.host_id).is_none();
            ctx.target = discovered
                .iter()
                .filter(unpaired)
                .find(|c| c.state == RpState::Ready)
                .or_else(|| discovered.iter().find(unpaired))
                .cloned();
        }
        let ui = self.ui();
        let g = ui.global::<PairingState>();
        match &ctx.target {
            Some(c) => {
                g.set_has_target(true);
                g.set_target_title(format!("Found {} on this network", c.name).into());
                let kind = if c.kind == rp_core::ConsoleKind::Ps5 {
                    "PS5"
                } else {
                    "PS4"
                };
                g.set_target_detail(format!("{kind} · {} · {}", c.addr, c.state).into());
            }
            None => {
                g.set_has_target(false);
                g.set_target_title("Looking for consoles on this network…".into());
                g.set_target_detail("Turn the console on, or enter its IP address".into());
            }
        }
    }

    fn set_pin(&self, pin: &str) {
        let ui = self.ui();
        let g = ui.global::<PairingState>();
        g.set_pin(pin.into());
        g.set_pin_digits(strings(model::pin_digits(pin)));
        g.set_pin_complete(pin.len() == 8);
    }

    fn submit_sign_in(&self, text: &str) {
        let ui = self.ui();
        let g = ui.global::<PairingState>();
        if g.get_busy() {
            return;
        }
        g.set_error("".into());
        match model::parse_sign_in(text) {
            Err(e) => g.set_error(e.into()),
            Ok(model::SignIn::AccountId(id)) => self.sign_in_done(Ok(id)),
            Ok(model::SignIn::Code(code)) => {
                g.set_busy(true);
                std::thread::spawn(move || {
                    let r = rp_core::psn::fetch_account_id(&code).map_err(|e| e.to_string());
                    post(move |app| app.sign_in_done(r));
                });
            }
        }
    }

    fn sign_in_done(&self, result: Result<rp_core::AccountId, String>) {
        let ui = self.ui();
        let g = ui.global::<PairingState>();
        g.set_busy(false);
        let id = match result {
            Ok(id) => id,
            Err(e) => {
                g.set_error(format!("Sign-in failed: {e}. The code in the address works once and only for a few minutes; sign in again for a new one.").into());
                return;
            }
        };
        if let Err(e) = self.store.borrow_mut().set_account_id(id) {
            g.set_error(format!("Could not save the account ID: {e}").into());
            return;
        }
        self.refresh_consoles();
        self.refresh_settings();
        if self.pairing.borrow().sign_in_only {
            self.close_pairing();
        } else {
            g.set_step(2);
        }
    }

    fn submit_pin(&self) {
        let ui = self.ui();
        let g = ui.global::<PairingState>();
        if g.get_busy() {
            return;
        }
        g.set_error("".into());
        let pin = match rp_core::parse_pin(&model::sanitize_pin(&g.get_pin())) {
            Ok(p) if g.get_pin().len() == 8 => p,
            _ => {
                g.set_error("The code has 8 digits.".into());
                return;
            }
        };
        let account_id = match self.store.borrow().account_id() {
            Ok(Some(id)) => id,
            _ => {
                g.set_step(1);
                return;
            }
        };
        let host = if g.get_manual_ip() {
            match crate::resolve(g.get_ip_text().trim()) {
                Ok(ip) => ip,
                Err(e) => {
                    g.set_error(format!("{e:#}").into());
                    return;
                }
            }
        } else {
            match &self.pairing.borrow().target {
                Some(c) => c.addr,
                None => {
                    g.set_error("No console found yet. Enter its IP address instead.".into());
                    return;
                }
            }
        };
        g.set_busy(true);
        std::thread::spawn(move || {
            let r = pair(host, account_id, pin);
            post(move |app| app.pair_done(r));
        });
    }

    fn pair_done(&self, result: Result<(RegisteredConsole, String, String), String>) {
        let ui = self.ui();
        let g = ui.global::<PairingState>();
        g.set_busy(false);
        let (registered, name, addr) = match result {
            Ok(r) => r,
            Err(e) => {
                g.set_error(e.into());
                return;
            }
        };
        match self
            .store
            .borrow_mut()
            .add_paired(&registered, &name, &addr)
        {
            Ok(p) => {
                g.set_linked_name(p.name.clone().into());
                self.pairing.borrow_mut().linked = Some(p.host_id);
                g.set_step(3);
            }
            Err(e) => g.set_error(format!("Linked, but the keys could not be saved: {e}").into()),
        }
        self.refresh_consoles();
        self.refresh_settings();
    }

    // ------------------------------------------------------------ connecting

    fn wire_connecting(self: &Rc<Self>) {
        let ui = self.ui();
        let g = ui.global::<ConnectingState>();
        let app = self.clone();
        g.on_cancel(move || app.stop_stream());
        let app = self.clone();
        g.on_submit_passcode(move |pin| {
            if let Some(s) = app.stream.borrow().as_ref() {
                s.stream.set_login_pin(pin.to_string());
            }
            let ui = app.ui();
            let g = ui.global::<ConnectingState>();
            g.set_passcode_requested(false);
            g.set_passcode("".into());
        });
    }

    fn connect(self: &Rc<Self>, host_id: &str) {
        if self.stream.borrow().is_some() {
            return;
        }
        let store = self.store.borrow();
        let Some(paired) = store.find(host_id).cloned() else {
            return;
        };
        let secrets = match store.secrets_for(&paired) {
            Ok(Some(s)) => s,
            Ok(None) => {
                drop(store);
                return self.banner(format!(
                    "The keys for {} are missing. Forget it in Settings › Account and link it again.",
                    paired.name
                ));
            }
            Err(e) => {
                drop(store);
                return self.banner(format!("Could not read the keys for {}: {e}", paired.name));
            }
        };
        let account_id = match store.account_id() {
            Ok(Some(id)) => id,
            _ => {
                drop(store);
                return self.banner("Sign in first: Settings › Account.");
            }
        };
        let settings = self.overrides.apply(store.config().stream);
        drop(store);

        let id = self.next_stream_id.get();
        self.next_stream_id.set(id + 1);
        let pending_redraw = Arc::new(AtomicBool::new(false));
        let sink: EventSink = Arc::new(move |ev: StreamEvent| {
            let is_frame = matches!(ev, StreamEvent::Frame);
            // One redraw request in flight is enough.
            if is_frame && pending_redraw.swap(true, Ordering::AcqRel) {
                return;
            }
            let pending = pending_redraw.clone();
            post(move |app| {
                if is_frame {
                    pending.store(false, Ordering::Release);
                }
                app.on_stream_event(id, ev);
            });
        });
        let request = StreamRequest {
            paired: paired.clone(),
            secrets,
            account_id,
            settings,
            play: self.overrides.play_options(),
        };
        let stream = match Stream::start(request, self.gpu.clone(), sink) {
            Ok(s) => s,
            Err(e) => return self.banner(format!("Could not start: {e}")),
        };
        let progress = ConnectProgress::new(Instant::now());

        let ui = self.ui();
        let g = ui.global::<ConnectingState>();
        g.set_console_name(paired.name.clone().into());
        g.set_console_kind(
            if paired.target().is_ps5() {
                "PS5"
            } else {
                "PS4"
            }
            .into(),
        );
        g.set_detail(model::connect_detail(&settings).into());
        g.set_error("".into());
        g.set_passcode_requested(false);
        g.set_passcode("".into());
        let s = ui.global::<SessionState>();
        s.set_console_name(paired.name.to_uppercase().into());
        s.set_has_video(false);
        s.set_video(slint::Image::default());
        s.set_menu_open(false);
        s.set_quality_text(model::quality_label(&settings).into());
        s.set_stats_text("Connecting…".into());

        *self.stream.borrow_mut() = Some(ActiveStream {
            id,
            stream,
            host_id: paired.host_id.clone(),
            settings,
            progress,
            meter: StatsMeter::default(),
            frames_lost: 0,
        });
        self.show_progress();
        self.show(Screen::Connecting);
    }

    fn show_progress(&self) {
        let stream = self.stream.borrow();
        if let Some(s) = stream.as_ref() {
            self.set_steps(&s.progress);
        }
    }

    fn set_steps(&self, progress: &ConnectProgress) {
        let ui = self.ui();
        let g = ui.global::<ConnectingState>();
        g.set_progress(progress.progress());
        g.set_steps(model(
            progress
                .steps
                .iter()
                .map(|st| StepItem {
                    title: st.title.into(),
                    detail: st.detail.clone().into(),
                    state: match st.state {
                        model::StepState::Waiting => StepState::Waiting,
                        model::StepState::Active => StepState::Active,
                        model::StepState::Done => StepState::Done,
                        model::StepState::Failed => StepState::Failed,
                    },
                })
                .collect(),
        ));
    }

    fn stop_stream(&self) {
        let ended = self.stream.borrow_mut().take();
        if let Some(s) = ended {
            if s.meter.fps > 0.0 {
                self.last_backend.set(Some(s.stream.stats().backend()));
            }
            s.stream.stop();
            self.stopping.borrow_mut().push((s.id, s.stream));
        }
        self.leave_session(Screen::Consoles);
    }

    fn leave_session(&self, to: Screen) {
        let ui = self.ui();
        if ui.window().is_fullscreen() {
            ui.window().set_fullscreen(false);
        }
        let s = ui.global::<SessionState>();
        s.set_fullscreen(false);
        s.set_video(slint::Image::default());
        s.set_has_video(false);
        self.refresh_consoles();
        self.show(to);
        if self.exit_after_stream {
            let _ = slint::quit_event_loop();
        }
    }

    fn on_stream_event(&self, id: u64, ev: StreamEvent) {
        if let StreamEvent::Ended(end) = ev {
            self.stopping.borrow_mut().retain(|(i, _)| *i != id);
            if let Some(summary) = end.summary
                && let Err(e) = self.store.borrow_mut().set_last_session(summary)
            {
                tracing::warn!("could not save session summary: {e}");
            }
            let current = self.stream.borrow().as_ref().is_some_and(|s| s.id == id);
            if !current {
                self.refresh_consoles();
                return;
            }
            let s = self.stream.borrow_mut().take().unwrap();
            if s.meter.fps > 0.0 {
                self.last_backend.set(Some(s.stream.stats().backend()));
            }
            if !end.error {
                return self.leave_session(Screen::Consoles);
            }
            if self.exit_after_stream {
                *self.exit_error.borrow_mut() = Some(format!("session ended: {}", end.reason));
            }
            if self.screen() == Screen::Connecting {
                // Stay on the connecting screen and show what failed.
                let mut progress = s.progress;
                progress.failed();
                *self.stream.borrow_mut() = None;
                let ui = self.ui();
                let g = ui.global::<ConnectingState>();
                g.set_error(capitalize(&end.reason).into());
                g.set_passcode_requested(false);
                self.set_steps(&progress);
                if self.exit_after_stream {
                    let _ = slint::quit_event_loop();
                }
            } else {
                self.banner(format!("Session ended: {}", end.reason));
                self.leave_session(Screen::Consoles);
            }
            return;
        }

        let mut guard = self.stream.borrow_mut();
        let Some(s) = guard.as_mut().filter(|s| s.id == id) else {
            return;
        };
        let now = Instant::now();
        let ui = self.ui();
        match ev {
            StreamEvent::Waking => s.progress.waking(),
            StreamEvent::Found { addr, woke } => {
                s.progress.found(now, &addr.to_string(), woke);
                let host_id = s.host_id.clone();
                if let Err(e) = self
                    .store
                    .borrow_mut()
                    .update_address(&host_id, &addr.to_string())
                {
                    tracing::warn!("could not save address: {e}");
                }
            }
            StreamEvent::Handshake => {}
            StreamEvent::LoginPinRequest { previous_incorrect } => {
                s.progress.passcode();
                let g = ui.global::<ConnectingState>();
                g.set_passcode_retry(previous_incorrect);
                g.set_passcode_requested(true);
            }
            StreamEvent::Connected => s.progress.connected(now),
            StreamEvent::Texture(texture) => match slint::Image::try_from(texture) {
                Ok(image) => {
                    let g = ui.global::<SessionState>();
                    g.set_video(image);
                    g.set_has_video(true);
                    if ui.get_screen() == Screen::Connecting {
                        s.progress.streaming(now);
                        drop(guard);
                        self.show(Screen::Session);
                        return;
                    }
                }
                Err(e) => tracing::error!("cannot show video texture: {e}"),
            },
            StreamEvent::Frame => {
                ui.window().request_redraw();
                return;
            }
            StreamEvent::Ended(_) => unreachable!(),
        }
        drop(guard);
        self.show_progress();
    }

    // ------------------------------------------------------------ session

    fn wire_session(self: &Rc<Self>) {
        let ui = self.ui();
        let g = ui.global::<SessionState>();
        let app = self.clone();
        g.on_disconnect(move || app.stop_stream());
        let app = self.clone();
        g.on_toggle_fullscreen(move || {
            let ui = app.ui();
            let full = !ui.window().is_fullscreen();
            ui.window().set_fullscreen(full);
            ui.global::<SessionState>().set_fullscreen(full);
        });
    }

    fn update_stats(&self) {
        self.stopping.borrow_mut().retain(|(_, s)| !s.is_finished());
        let mut guard = self.stream.borrow_mut();
        let Some(s) = guard.as_mut() else { return };
        let stats = s.stream.stats();
        s.meter.update(
            Instant::now(),
            stats.presented.load(Ordering::Relaxed),
            stats.bytes.load(Ordering::Relaxed),
        );
        let lost = stats.frames_lost.load(Ordering::Relaxed);
        let good = lost == s.frames_lost && s.meter.fps > 0.0;
        s.frames_lost = lost;
        let text = format!(
            "{} · {} · {}",
            s.meter.label(),
            s.settings.codec.label(),
            stats.backend()
        );
        drop(guard);
        let ui = self.ui();
        let g = ui.global::<SessionState>();
        g.set_stats_text(text.into());
        g.set_stats_good(good);
    }

    // ------------------------------------------------------------ settings

    fn wire_settings(self: &Rc<Self>) {
        let ui = self.ui();
        let g = ui.global::<SettingsState>();
        let app = self.clone();
        g.on_set_resolution(move |i| {
            app.update_settings(|s| {
                if let Some(r) = VideoResolution::ALL.get(i as usize) {
                    s.resolution = *r;
                    s.bitrate_kbps = None;
                }
            })
        });
        let app = self.clone();
        g.on_set_fps(move |i| app.update_settings(|s| s.fps = if i == 0 { 30 } else { 60 }));
        let app = self.clone();
        g.on_set_codec(move |i| {
            app.update_settings(|s| {
                s.codec = if i == 0 {
                    VideoCodec::H264
                } else {
                    VideoCodec::H265
                }
            })
        });
        let app = self.clone();
        g.on_set_bitrate(move |mbps| {
            app.update_settings(|s| s.bitrate_kbps = Some((mbps.round() as u32) * 1000))
        });
        let app = self.clone();
        g.on_reset_bitrate(move || app.update_settings(|s| s.bitrate_kbps = None));
        let app = self.clone();
        g.on_set_decoder(move |i| {
            app.update_settings(|s| {
                s.decoder = if i == 0 {
                    DecoderPreference::Auto
                } else {
                    DecoderPreference::Software
                }
            })
        });
        let app = self.clone();
        g.on_sign_in(move || app.open_pairing(None, true, Screen::Settings));
        let app = self.clone();
        g.on_forget(move |id| {
            let r = app.store.borrow_mut().remove(&id);
            if let Err(e) = r {
                tracing::error!("could not forget console: {e}");
            }
            app.refresh_settings();
            app.refresh_consoles();
        });
        let app = self.clone();
        g.on_back(move || app.show(app.settings_return.get()));
    }

    fn open_settings(&self, section: i32) {
        let from = self.screen();
        if from != Screen::Settings {
            self.settings_return.set(from);
        }
        self.ui().global::<SettingsState>().set_section(section);
        self.refresh_settings();
        self.show(Screen::Settings);
    }

    fn update_settings(&self, f: impl FnOnce(&mut StreamSettings)) {
        let mut s = self.settings();
        f(&mut s);
        if let Err(e) = self.store.borrow_mut().set_stream_settings(s) {
            tracing::error!("could not save settings: {e}");
        }
        self.refresh_settings();
        self.refresh_consoles();
    }

    fn refresh_settings(&self) {
        let ui = self.ui();
        let g = ui.global::<SettingsState>();
        let store = self.store.borrow();
        let s = store.config().stream;
        g.set_resolutions(strings(VideoResolution::ALL.iter().map(|r| r.label())));
        g.set_resolution(
            VideoResolution::ALL
                .iter()
                .position(|r| *r == s.resolution)
                .unwrap_or(0) as i32,
        );
        g.set_fps(if s.fps() == 30 { 0 } else { 1 });
        g.set_codec(model::codec_index(s.codec));
        g.set_bitrate_min(BITRATE_RANGE_KBPS.0 as f32 / 1000.0);
        g.set_bitrate_max(BITRATE_RANGE_KBPS.1 as f32 / 1000.0);
        g.set_bitrate_mbps(s.video_profile().bitrate as f32 / 1000.0);
        let auto = match Backend::platform_default() {
            Some(hw) if self.hw_available.get() != Some(false) => format!("Auto · {hw}"),
            _ => "Auto · software".into(),
        };
        g.set_decoders(strings([auto, "Software".into()]));
        g.set_decoder(if s.decoder == DecoderPreference::Auto {
            0
        } else {
            1
        });
        g.set_estimate(model::estimate(&s).into());

        g.set_account_id(
            store
                .config()
                .psn_account_id
                .clone()
                .unwrap_or_default()
                .into(),
        );
        g.set_linked(model(
            store
                .paired()
                .iter()
                .map(|p| LinkedConsole {
                    host_id: p.host_id.clone().into(),
                    name: p.name.clone().into(),
                    meta: format!(
                        "{} · {} · {}",
                        if p.target().is_ps5() { "PS5" } else { "PS4" },
                        p.address,
                        p.host_id
                    )
                    .into(),
                })
                .collect(),
        ));
        g.set_storage(
            format!(
                "Settings are in {}. Console keys are in the {}.",
                store.config_path().display(),
                store.secret_store().describe()
            )
            .into(),
        );
        g.set_version(env!("CARGO_PKG_VERSION").into());
    }
}

/// Discover → register, on a worker thread. Returns the registration, the
/// console's name and its address.
fn pair(
    host: std::net::IpAddr,
    account_id: rp_core::AccountId,
    pin: u32,
) -> Result<(RegisteredConsole, String, String), String> {
    let options = DiscoveryOptions {
        host: Some(host),
        ..Default::default()
    };
    let found = rp_core::discover(Duration::from_secs(3), &options)
        .map_err(|e| e.to_string())?
        .into_iter()
        .next()
        .ok_or_else(|| {
            format!("No console answered at {host}. Is it on and on the same network?")
        })?;
    if found.state == RpState::Standby {
        return Err(format!(
            "{} is in rest mode. Turn it on, then open {PAIR_SCREEN}.",
            found.name
        ));
    }
    let registered = rp_core::register(&RegistRequest {
        host: host.to_string(),
        target: found.target,
        account_id,
        pin,
    })
    .map_err(|e| {
        format!(
            "Linking failed: {e}. Check the code; it changes each time {PAIR_SCREEN} is opened."
        )
    })?;
    Ok((registered, found.name, host.to_string()))
}

fn start_discovery() {
    let spawned = std::thread::Builder::new()
        .name("discovery".into())
        .spawn(|| {
            let options = DiscoveryOptions {
                ping_interval: Duration::from_secs(1),
                drop_after_missed_pings: 5,
                ..Default::default()
            };
            let discovery = match rp_core::Discovery::start(&options) {
                Ok(d) => d,
                Err(e) => {
                    let msg = format!("Could not search the network for consoles: {e}");
                    post(move |app| app.banner(msg));
                    return;
                }
            };
            for list in discovery.updates().iter() {
                post(move |app| app.set_discovered(list));
            }
        });
    if let Err(e) = spawned {
        tracing::error!("could not start discovery: {e}");
    }
}

/// Checks once whether the hardware decoder can be opened, for the labels.
fn probe_hardware() {
    let Some(hw) = Backend::platform_default() else {
        return;
    };
    let _ = std::thread::Builder::new()
        .name("hw-probe".into())
        .spawn(move || {
            let ok = crate::video::hwaccel::probe(hw);
            post(move |app| {
                app.hw_available.set(Some(ok));
                app.refresh_consoles();
                app.refresh_settings();
            });
        });
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}
