//! View-model logic for the UI, kept free of Slint types so it can be tested.

use std::time::{Duration, Instant};

use rp_core::{AccountId, Console, ConsoleKind, ConsoleState};
use tether_core::{LastSession, PairedConsole, StreamSettings, VideoCodec};

// ---------------------------------------------------------------- consoles

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RowState {
    Ready,
    Rest,
    /// Paired, but not answering on this network.
    Away,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsoleRow {
    pub host_id: String,
    pub name: String,
    pub meta: String,
    pub state: RowState,
    pub paired: bool,
    pub featured: bool,
}

fn kind_label(kind: ConsoleKind) -> &'static str {
    match kind {
        ConsoleKind::Ps5 => "PS5",
        ConsoleKind::Ps4 => "PS4",
    }
}

fn row_state(state: ConsoleState) -> RowState {
    match state {
        ConsoleState::Ready => RowState::Ready,
        ConsoleState::Standby => RowState::Rest,
        ConsoleState::Unknown => RowState::Unknown,
    }
}

/// Paired consoles first (on, then in rest mode, then away), then consoles
/// on the network that aren't paired yet. The first row is shown large if
/// it's a paired console that is on.
pub fn build_rows(paired: &[PairedConsole], discovered: &[Console]) -> Vec<ConsoleRow> {
    let found = |host_id: &str| {
        discovered
            .iter()
            .find(|c| c.host_id.eq_ignore_ascii_case(host_id))
    };
    let mut rows: Vec<ConsoleRow> = paired
        .iter()
        .map(|p| {
            let kind = if p.target().is_ps5() { "PS5" } else { "PS4" };
            match found(&p.host_id) {
                Some(c) => ConsoleRow {
                    host_id: p.host_id.clone(),
                    name: p.name.clone(),
                    meta: format!("{} · {}", kind_label(c.kind), c.addr),
                    state: row_state(c.state),
                    paired: true,
                    featured: false,
                },
                None => ConsoleRow {
                    host_id: p.host_id.clone(),
                    name: p.name.clone(),
                    meta: format!("{kind} · last seen at {}", p.address),
                    state: RowState::Away,
                    paired: true,
                    featured: false,
                },
            }
        })
        .collect();
    rows.sort_by(|a, b| a.state.cmp(&b.state).then_with(|| a.name.cmp(&b.name)));

    let mut unpaired: Vec<ConsoleRow> = discovered
        .iter()
        .filter(|c| {
            !paired
                .iter()
                .any(|p| p.host_id.eq_ignore_ascii_case(&c.host_id))
        })
        .map(|c| ConsoleRow {
            host_id: c.host_id.clone(),
            name: c.name.clone(),
            meta: format!("{} · {}", kind_label(c.kind), c.addr),
            state: row_state(c.state),
            paired: false,
            featured: false,
        })
        .collect();
    unpaired.sort_by(|a, b| a.name.cmp(&b.name));
    rows.extend(unpaired);

    if let Some(first) = rows.first_mut()
        && first.paired
        && first.state == RowState::Ready
    {
        first.featured = true;
    }
    rows
}

/// "2 linked · 1 on this network"
pub fn summary(rows: &[ConsoleRow]) -> String {
    let linked = rows.iter().filter(|r| r.paired).count();
    let here = rows.iter().filter(|r| r.state != RowState::Away).count();
    match (linked, here) {
        (0, 0) => "Nothing linked yet".into(),
        (0, n) => format!("{n} on this network, none linked yet"),
        (l, n) => format!("{l} linked · {n} on this network"),
    }
}

// ---------------------------------------------------------------- pairing

/// Digits only, at most 8.
pub fn sanitize_pin(input: &str) -> String {
    input.chars().filter(char::is_ascii_digit).take(8).collect()
}

/// One string per box.
pub fn pin_digits(pin: &str) -> Vec<String> {
    let mut digits: Vec<String> = pin.chars().take(8).map(String::from).collect();
    digits.resize(8, String::new());
    digits
}

#[derive(Debug, PartialEq, Eq)]
pub enum SignIn {
    /// The user pasted an account ID (e.g. from chiaki-ng).
    AccountId(AccountId),
    /// The user pasted the redirect address; this is the code in it.
    Code(String),
}

/// What the user pasted on the sign-in step.
pub fn parse_sign_in(input: &str) -> Result<SignIn, String> {
    let input = input.trim();
    if input.is_empty() {
        return Err("Paste the address the browser ended on.".into());
    }
    if !input.contains("://")
        && !input.contains('?')
        && let Ok(id) = input.parse::<AccountId>()
    {
        return Ok(SignIn::AccountId(id));
    }
    rp_core::psn::parse_redirect(input)
        .map(SignIn::Code)
        .map_err(|e| e.to_string())
}

// ---------------------------------------------------------------- settings and labels

/// "720p · 60 fps · H.264"
pub fn stream_label(s: &StreamSettings) -> String {
    format!("{} · {} fps · {}", s.resolution, s.fps(), s.codec.label())
}

/// "720p · 60"
pub fn quality_label(s: &StreamSettings) -> String {
    format!("{} · {}", s.resolution, s.fps())
}

pub fn mbps(kbps: u32) -> String {
    let m = kbps as f64 / 1000.0;
    if (m - m.round()).abs() < 0.05 {
        format!("{m:.0} Mbps")
    } else {
        format!("{m:.1} Mbps")
    }
}

/// "Agreeing on 720p · 60 fps · H.264 · 10 Mbps"
pub fn connect_detail(s: &StreamSettings) -> String {
    format!(
        "Agreeing on {} · {}",
        stream_label(s),
        mbps(s.video_profile().bitrate)
    )
}

/// Data used per hour at the configured bitrate.
pub fn gigabytes_per_hour(kbps: u32) -> f64 {
    kbps as f64 * 3600.0 / 8.0 / 1_000_000.0
}

/// "720p · 60 fps · H.264 · ~4.5 GB per hour"
pub fn estimate(s: &StreamSettings) -> String {
    format!(
        "{} · ~{:.1} GB per hour",
        stream_label(s),
        gigabytes_per_hour(s.video_profile().bitrate)
    )
}

pub fn codec_index(c: VideoCodec) -> i32 {
    match c {
        VideoCodec::H264 => 0,
        VideoCodec::H265 => 1,
    }
}

/// "1 h 42 min", "12 min", "40 s"
pub fn format_duration(secs: u64) -> String {
    let (h, m) = (secs / 3600, (secs % 3600) / 60);
    match (h, m) {
        (0, 0) => format!("{secs} s"),
        (0, m) => format!("{m} min"),
        (h, 0) => format!("{h} h"),
        (h, m) => format!("{h} h {m} min"),
    }
}

/// "Today", "Yesterday", "3 days ago" (by elapsed time, not calendar days).
pub fn relative_day(then_unix: u64, now_unix: u64) -> String {
    match now_unix.saturating_sub(then_unix) / 86_400 {
        0 => "Today".into(),
        1 => "Yesterday".into(),
        n => format!("{n} days ago"),
    }
}

/// Title and detail lines of the "last session" card.
pub fn last_session_lines(s: &LastSession, now_unix: u64) -> (String, String) {
    (
        format!(
            "{} · {} · {}",
            relative_day(s.started_at, now_unix),
            s.console,
            format_duration(s.duration_secs)
        ),
        format!("{} · {} frames lost", s.video, s.frames_lost),
    )
}

// ---------------------------------------------------------------- connecting

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepState {
    Waiting,
    Active,
    Done,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    pub title: &'static str,
    pub detail: String,
    pub state: StepState,
}

/// The four cards on the connecting screen: Wake, Find, Handshake, Stream.
#[derive(Debug, Clone)]
pub struct ConnectProgress {
    pub steps: [Step; 4],
    phase_started: Instant,
}

fn secs(d: Duration) -> String {
    format!("{:.1} s", d.as_secs_f64())
}

impl ConnectProgress {
    pub fn new(now: Instant) -> Self {
        let step = |title, state, detail: &str| Step {
            title,
            detail: detail.into(),
            state,
        };
        Self {
            steps: [
                step("Wake", StepState::Active, "checking"),
                step("Find", StepState::Waiting, "waiting"),
                step("Handshake", StepState::Waiting, "waiting"),
                step("Stream", StepState::Waiting, "waiting"),
            ],
            phase_started: now,
        }
    }

    fn set(&mut self, i: usize, state: StepState, detail: impl Into<String>) {
        self.steps[i].state = state;
        self.steps[i].detail = detail.into();
    }

    pub fn waking(&mut self) {
        self.set(0, StepState::Active, "waking up");
    }

    pub fn found(&mut self, now: Instant, addr: &str, woke: bool) {
        let took = secs(now - self.phase_started);
        self.set(
            0,
            StepState::Done,
            if woke { took } else { "already on".into() },
        );
        self.set(1, StepState::Done, addr);
        self.set(2, StepState::Active, "in progress");
        self.phase_started = now;
    }

    pub fn connected(&mut self, now: Instant) {
        self.set(2, StepState::Done, secs(now - self.phase_started));
        self.set(3, StepState::Active, "first frame");
        self.phase_started = now;
    }

    pub fn streaming(&mut self, now: Instant) {
        self.set(3, StepState::Done, secs(now - self.phase_started));
    }

    pub fn passcode(&mut self) {
        self.set(2, StepState::Active, "passcode needed");
    }

    /// Marks the step in progress as failed.
    pub fn failed(&mut self) {
        if let Some(s) = self.steps.iter_mut().find(|s| s.state == StepState::Active) {
            s.state = StepState::Failed;
            s.detail = "failed".into();
        }
    }

    /// Position of the dot on the line, 0..1.
    pub fn progress(&self) -> f32 {
        let done = self
            .steps
            .iter()
            .filter(|s| s.state == StepState::Done)
            .count();
        let active = self.steps.iter().any(|s| s.state == StepState::Active);
        ((done as f32 + if active { 0.5 } else { 0.0 }) / 4.0).clamp(0.02, 1.0)
    }
}

// ---------------------------------------------------------------- live stats

/// Frame rate and bitrate from the decoder's running counters.
#[derive(Default)]
pub struct StatsMeter {
    last: Option<(Instant, u64, u64)>,
    pub fps: f64,
    pub mbps: f64,
}

impl StatsMeter {
    /// Frames shown and `bytes` received so far.
    pub fn update(&mut self, now: Instant, frames: u64, bytes: u64) {
        if let Some((t, d, b)) = self.last {
            let dt = (now - t).as_secs_f64();
            if dt < 0.2 {
                return;
            }
            self.fps = frames.saturating_sub(d) as f64 / dt;
            self.mbps = bytes.saturating_sub(b) as f64 * 8.0 / dt / 1_000_000.0;
        }
        self.last = Some((now, frames, bytes));
    }

    /// "60 fps · 9.8 Mbps"
    pub fn label(&self) -> String {
        format!("{:.0} fps · {:.1} Mbps", self.fps, self.mbps)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rp_core::Target;
    use tether_core::VideoResolution;

    fn paired(id: &str, name: &str) -> PairedConsole {
        PairedConsole {
            host_id: id.into(),
            name: name.into(),
            address: "192.168.1.9".into(),
            target: Target::PS5_1.raw(),
        }
    }

    fn found(id: &str, name: &str, ip: &str, state: ConsoleState) -> Console {
        Console {
            host_id: id.into(),
            name: name.into(),
            addr: ip.parse().unwrap(),
            state,
            kind: ConsoleKind::Ps5,
            target: Target::PS5_1,
            host_type: None,
            system_version: None,
            request_port: 997,
            running_app: None,
        }
    }

    #[test]
    fn rows_order_and_featured() {
        let paired = [
            paired("AA", "Office"),
            paired("BB", "Bedroom"),
            paired("CC", "Living Room"),
        ];
        let discovered = [
            found("bb", "Bedroom", "192.168.1.57", ConsoleState::Standby),
            found("CC", "PS5-309", "192.168.1.42", ConsoleState::Ready),
            found("DD", "Friend's", "192.168.1.60", ConsoleState::Ready),
        ];
        let rows = build_rows(&paired, &discovered);
        let names: Vec<_> = rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["Living Room", "Bedroom", "Office", "Friend's"]);
        assert!(rows[0].featured);
        assert!(rows[1..].iter().all(|r| !r.featured));
        assert_eq!(rows[0].meta, "PS5 · 192.168.1.42");
        assert_eq!(rows[1].state, RowState::Rest);
        assert_eq!(rows[2].state, RowState::Away);
        assert_eq!(rows[2].meta, "PS5 · last seen at 192.168.1.9");
        assert!(!rows[3].paired);
        assert_eq!(summary(&rows), "3 linked · 3 on this network");
    }

    #[test]
    fn nothing_featured_unless_paired_and_ready() {
        let rows = build_rows(&[], &[found("DD", "New", "10.0.0.2", ConsoleState::Ready)]);
        assert!(!rows[0].featured);
        assert_eq!(summary(&rows), "1 on this network, none linked yet");
        assert_eq!(summary(&[]), "Nothing linked yet");

        let rows = build_rows(
            &[paired("AA", "Den")],
            &[found("AA", "Den", "10.0.0.3", ConsoleState::Standby)],
        );
        assert!(!rows[0].featured);
    }

    #[test]
    fn pin_entry() {
        assert_eq!(sanitize_pin("3816-04 7a29"), "38160472");
        assert_eq!(sanitize_pin("12"), "12");
        let d = pin_digits("123");
        assert_eq!(d.len(), 8);
        assert_eq!(d[2], "3");
        assert_eq!(d[3], "");
    }

    #[test]
    fn sign_in_input() {
        let id = AccountId::from_user_id(42);
        assert_eq!(
            parse_sign_in(&format!(" {} ", id.to_base64())),
            Ok(SignIn::AccountId(id))
        );
        assert_eq!(
            parse_sign_in(
                "https://remoteplay.dl.playstation.net/remoteplay/redirect?code=abc123&cid=x"
            ),
            Ok(SignIn::Code("abc123".into()))
        );
        assert!(parse_sign_in("").is_err());
    }

    #[test]
    fn labels() {
        let s = StreamSettings {
            resolution: VideoResolution::P1080,
            fps: 60,
            codec: VideoCodec::H265,
            bitrate_kbps: Some(30_000),
            ..Default::default()
        };
        assert_eq!(stream_label(&s), "1080p · 60 fps · HEVC");
        assert_eq!(quality_label(&s), "1080p · 60");
        assert_eq!(
            connect_detail(&s),
            "Agreeing on 1080p · 60 fps · HEVC · 30 Mbps"
        );
        assert_eq!(estimate(&s), "1080p · 60 fps · HEVC · ~13.5 GB per hour");
        assert_eq!(mbps(10_000), "10 Mbps");
        assert_eq!(mbps(7_500), "7.5 Mbps");
    }

    #[test]
    fn durations_and_days() {
        assert_eq!(format_duration(40), "40 s");
        assert_eq!(format_duration(12 * 60 + 5), "12 min");
        assert_eq!(format_duration(3600), "1 h");
        assert_eq!(format_duration(6120), "1 h 42 min");
        let now = 1_700_000_000;
        assert_eq!(relative_day(now - 60, now), "Today");
        assert_eq!(relative_day(now - 90_000, now), "Yesterday");
        assert_eq!(relative_day(now - 3 * 86_400, now), "3 days ago");
        let last = LastSession {
            console: "Living Room".into(),
            started_at: now - 90_000,
            duration_secs: 6120,
            video: "1080p60".into(),
            frames_lost: 0,
        };
        assert_eq!(
            last_session_lines(&last, now),
            (
                "Yesterday · Living Room · 1 h 42 min".to_string(),
                "1080p60 · 0 frames lost".to_string()
            )
        );
    }

    #[test]
    fn connect_steps() {
        let t0 = Instant::now();
        let mut p = ConnectProgress::new(t0);
        assert_eq!(p.steps[0].state, StepState::Active);
        p.waking();
        p.found(t0 + Duration::from_millis(800), "192.168.1.42", true);
        assert_eq!(p.steps[0].detail, "0.8 s");
        assert_eq!(p.steps[1].state, StepState::Done);
        assert_eq!(p.steps[2].state, StepState::Active);
        assert!((p.progress() - 2.5 / 4.0).abs() < 1e-6);
        p.failed();
        assert_eq!(p.steps[2].state, StepState::Failed);

        let mut p = ConnectProgress::new(t0);
        p.found(t0, "10.0.0.1", false);
        assert_eq!(p.steps[0].detail, "already on");
        p.connected(t0 + Duration::from_millis(1500));
        assert_eq!(p.steps[2].detail, "1.5 s");
        p.streaming(t0 + Duration::from_secs(2));
        assert!(p.steps.iter().all(|s| s.state == StepState::Done));
        assert_eq!(p.progress(), 1.0);
    }

    #[test]
    fn stats_meter() {
        let t0 = Instant::now();
        let mut m = StatsMeter::default();
        m.update(t0, 0, 0);
        m.update(t0 + Duration::from_millis(100), 6, 1000); // too soon, ignored
        m.update(t0 + Duration::from_secs(1), 60, 1_250_000);
        assert!((m.fps - 60.0).abs() < 1e-9);
        assert!((m.mbps - 10.0).abs() < 1e-9);
        assert_eq!(m.label(), "60 fps · 10.0 Mbps");
    }
}
