//! Gamepads through SDL3: input goes to the console as soon as it arrives,
//! and the console's feedback (rumble, adaptive triggers, light bar, player
//! lights) goes back to the controllers.
//!
//! SDL runs on its own thread with only the gamepad subsystem; it never
//! owns a window (winit does), so it's told to keep reading controllers
//! while it has no focused window of its own.

use std::collections::HashMap;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use anyhow::{Result, anyhow};
use crossbeam_channel::{Receiver, Sender, TryRecvError};
use rp_core::controller::{TOUCHPAD_HEIGHT, TOUCHPAD_WIDTH};
use rp_core::{Buttons, ControllerState, EffectIntensity, MotionTracker, Session};
use sdl3::event::Event;
use sdl3::gamepad::{Axis, Button, Gamepad, GamepadType};
use sdl3::joystick::JoystickId;
use sdl3::sensor::SensorType;

/// Standard gravity, for converting SDL's m/s² to g.
const STANDARD_GRAVITY: f32 = 9.80665;
/// How long one rumble command lasts if the console never stops it.
const RUMBLE_MS: u32 = 5000;
/// How often the SDL thread checks for feedback when no input arrives.
const POLL: Duration = Duration::from_millis(4);

/// Console → controller feedback.
#[derive(Debug, Clone, PartialEq)]
pub enum Feedback {
    /// Motor strengths on SDL's 0..=65535 scale.
    Rumble {
        low: u16,
        high: u16,
    },
    TriggerEffects {
        type_left: u8,
        left: [u8; 10],
        type_right: u8,
        right: [u8; 10],
    },
    Led([u8; 3]),
    PlayerIndex(u8),
    MotionReset,
    HapticIntensity(EffectIntensity),
    TriggerIntensity(EffectIntensity),
}

impl Feedback {
    /// The feedback a session event asks for, if any.
    pub fn from_event(ev: &rp_core::SessionEvent) -> Option<Self> {
        use rp_core::SessionEvent as E;
        Some(match *ev {
            E::Rumble { left, right } => Feedback::Rumble {
                low: (left as u16) << 8,
                high: (right as u16) << 8,
            },
            E::TriggerEffects {
                type_left,
                left,
                type_right,
                right,
            } => Feedback::TriggerEffects {
                type_left,
                left,
                type_right,
                right,
            },
            E::LedColor(rgb) => Feedback::Led(rgb),
            E::PlayerIndex(i) => Feedback::PlayerIndex(i),
            E::MotionReset => Feedback::MotionReset,
            E::HapticIntensity(i) => Feedback::HapticIntensity(i),
            E::TriggerIntensity(i) => Feedback::TriggerIntensity(i),
            _ => return None,
        })
    }
}

/// The gamepad thread. Stops when dropped.
pub struct Gamepads {
    quit: Sender<()>,
    thread: Option<JoinHandle<()>>,
}

impl Gamepads {
    /// Starts SDL and sends every controller change to `session`, and
    /// applies everything that arrives on `feedback` to the controllers.
    /// `on_mic_button` runs (on the gamepad thread) when a DualSense's mute
    /// button is pressed. `on_pads` gets the names of the connected
    /// controllers at start and whenever one is added or removed.
    pub fn start(
        session: Arc<Session>,
        feedback: Receiver<Feedback>,
        on_mic_button: impl FnMut() + Send + 'static,
        on_pads: impl FnMut(Vec<String>) + Send + 'static,
    ) -> Result<Self> {
        let (quit, quit_rx) = crossbeam_channel::bounded(1);
        let (ready_tx, ready_rx) = crossbeam_channel::bounded(1);
        let thread = std::thread::Builder::new()
            .name("gamepads".into())
            .spawn(move || {
                let mut sdl =
                    match SdlInput::new(session, Box::new(on_mic_button), Box::new(on_pads)) {
                        Ok(s) => {
                            let _ = ready_tx.send(Ok(()));
                            s
                        }
                        Err(e) => {
                            let _ = ready_tx.send(Err(e));
                            return;
                        }
                    };
                sdl.run(&feedback, &quit_rx);
            })?;
        ready_rx
            .recv()
            .map_err(|_| anyhow!("gamepad thread died"))??;
        Ok(Self {
            quit,
            thread: Some(thread),
        })
    }
}

impl Drop for Gamepads {
    fn drop(&mut self) {
        let _ = self.quit.send(());
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// One open controller and the state it reports.
struct Pad {
    gamepad: Gamepad,
    is_dualsense: bool,
    state: ControllerState,
    motion: MotionTracker,
    /// SDL (touchpad, finger) → chiaki touch id.
    touches: HashMap<(i32, i32), u8>,
}

struct SdlInput {
    // Field order matters for drop: pads close before SDL shuts down.
    pads: HashMap<JoystickId, Pad>,
    subsystem: sdl3::GamepadSubsystem,
    pump: sdl3::EventPump,
    _sdl: sdl3::Sdl,
    session: Arc<Session>,
    on_mic_button: Box<dyn FnMut() + Send>,
    on_pads: Box<dyn FnMut(Vec<String>) + Send>,
    /// Last state sent to the console.
    sent: ControllerState,
    rumble_scale: f32,
    /// The intensity byte DualSense effect reports carry (chiaki-ng's
    /// encoding: trigger nibble | rumble nibble).
    trigger_intensity: Option<u8>,
    rumble_intensity: Option<u8>,
    led: Option<[u8; 3]>,
    player_index: Option<u8>,
}

impl SdlInput {
    fn new(
        session: Arc<Session>,
        on_mic_button: Box<dyn FnMut() + Send>,
        on_pads: Box<dyn FnMut(Vec<String>) + Send>,
    ) -> Result<Self> {
        // No SDL window, so SDL never has focus: read controllers anyway.
        sdl3::hint::set("SDL_JOYSTICK_ALLOW_BACKGROUND_EVENTS", "1");
        let sdl = sdl3::init().map_err(|e| anyhow!("SDL init: {e}"))?;
        let subsystem = sdl
            .gamepad()
            .map_err(|e| anyhow!("SDL gamepad init: {e}"))?;
        let pump = sdl.event_pump().map_err(|e| anyhow!("SDL events: {e}"))?;
        let mut input = Self {
            pads: HashMap::new(),
            subsystem,
            pump,
            _sdl: sdl,
            session,
            on_mic_button,
            on_pads,
            sent: ControllerState::default(),
            rumble_scale: 1.0,
            trigger_intensity: Some(0x00),
            rumble_intensity: Some(0x00),
            led: None,
            player_index: None,
        };
        let ids = input
            .subsystem
            .gamepads()
            .map_err(|e| anyhow!("listing gamepads: {e}"))?;
        for id in ids {
            input.open(id);
        }
        if input.pads.is_empty() {
            eprintln!("No controller found; connect one at any time.");
        }
        input.report_pads();
        Ok(input)
    }

    /// Tells `on_pads` which controllers are connected now.
    fn report_pads(&mut self) {
        let mut names: Vec<String> = self
            .pads
            .values()
            .map(|p| p.gamepad.name().unwrap_or_else(|| "Controller".into()))
            .collect();
        names.sort();
        (self.on_pads)(names);
    }

    fn run(&mut self, feedback: &Receiver<Feedback>, quit: &Receiver<()>) {
        loop {
            if let Some(ev) = self.pump.wait_event_timeout(POLL) {
                let mut changed = self.handle(ev);
                // Take everything already queued before sending, so a burst
                // of axis events becomes one update.
                while let Some(ev) = self.pump.poll_event() {
                    changed |= self.handle(ev);
                }
                if changed {
                    self.send_state();
                }
            }
            while let Ok(f) = feedback.try_recv() {
                self.feedback(f);
            }
            if !matches!(quit.try_recv(), Err(TryRecvError::Empty)) {
                self.reset_feedback();
                return;
            }
        }
    }

    fn open(&mut self, id: JoystickId) {
        if self.pads.contains_key(&id) {
            return;
        }
        let gamepad = match self.subsystem.open(id) {
            Ok(g) => g,
            Err(e) => {
                tracing::warn!("can't open gamepad {id:?}: {e}");
                return;
            }
        };
        let name = gamepad.name().unwrap_or_else(|| "controller".into());
        let is_dualsense = gamepad.r#type() == GamepadType::PS5;
        let mut motion = false;
        for sensor in [SensorType::Gyroscope, SensorType::Accelerometer] {
            motion |= gamepad.sensor_set_enabled(sensor, true).is_ok();
        }
        eprintln!(
            "Controller connected: {name}{}",
            if motion { " (motion)" } else { "" }
        );
        let mut pad = Pad {
            gamepad,
            is_dualsense,
            state: ControllerState::default(),
            motion: MotionTracker::new(),
            touches: HashMap::new(),
        };
        if let Some([r, g, b]) = self.led {
            let _ = pad.gamepad.set_led(r, g, b);
        }
        if let Some(i) = self.player_index {
            let _ = pad.gamepad.set_player_index(i as u16);
        }
        self.pads.insert(id, pad);
    }

    /// Applies one SDL event. Returns whether the controller state changed.
    fn handle(&mut self, ev: Event) -> bool {
        match ev {
            Event::GamepadAdded { which, .. } => {
                if !self.pads.contains_key(&which) {
                    self.open(which);
                    self.report_pads();
                }
                false
            }
            Event::GamepadRemoved { which, .. } => {
                if let Some(p) = self.pads.remove(&which) {
                    eprintln!(
                        "Controller disconnected: {}",
                        p.gamepad.name().unwrap_or_default()
                    );
                    self.report_pads();
                }
                true
            }
            Event::GamepadButtonDown { which, button, .. } => self.button(which, button, true),
            Event::GamepadButtonUp { which, button, .. } => self.button(which, button, false),
            Event::GamepadAxisMotion {
                which, axis, value, ..
            } => {
                let Some(pad) = self.pads.get_mut(&which) else {
                    return false;
                };
                apply_axis(&mut pad.state, axis, value);
                true
            }
            Event::GamepadTouchpadDown {
                which,
                touchpad,
                finger,
                x,
                y,
                ..
            } => {
                let Some(pad) = self.pads.get_mut(&which) else {
                    return false;
                };
                let (x, y) = touch_pos(x, y);
                match pad.state.start_touch(x, y) {
                    Some(id) => {
                        pad.touches.insert((touchpad, finger), id);
                        true
                    }
                    None => false,
                }
            }
            Event::GamepadTouchpadMotion {
                which,
                touchpad,
                finger,
                x,
                y,
                ..
            } => {
                let Some(pad) = self.pads.get_mut(&which) else {
                    return false;
                };
                let Some(&id) = pad.touches.get(&(touchpad, finger)) else {
                    return false;
                };
                let (x, y) = touch_pos(x, y);
                pad.state.move_touch(id, x, y);
                true
            }
            Event::GamepadTouchpadUp {
                which,
                touchpad,
                finger,
                ..
            } => {
                let Some(pad) = self.pads.get_mut(&which) else {
                    return false;
                };
                match pad.touches.remove(&(touchpad, finger)) {
                    Some(id) => {
                        pad.state.stop_touch(id);
                        true
                    }
                    None => false,
                }
            }
            Event::GamepadSensorUpdated {
                timestamp,
                which,
                sensor,
                data,
            } => {
                let Some(pad) = self.pads.get_mut(&which) else {
                    return false;
                };
                // Event timestamps are nanoseconds; chiaki wants wrapping µs.
                let us = (timestamp / 1000) as u32;
                match sensor {
                    SensorType::Gyroscope => pad.motion.update_gyro(data, us),
                    SensorType::Accelerometer => pad
                        .motion
                        .update_accel(data.map(|v| v / STANDARD_GRAVITY), us),
                    _ => return false,
                }
                pad.motion.apply(&mut pad.state);
                true
            }
            _ => false,
        }
    }

    fn button(&mut self, which: JoystickId, button: Button, down: bool) -> bool {
        if button == Button::Misc1 {
            // The DualSense mute button toggles our microphone instead of
            // being sent on.
            if down {
                (self.on_mic_button)();
            }
            return false;
        }
        let Some(pad) = self.pads.get_mut(&which) else {
            return false;
        };
        match map_button(button) {
            Some(b) => {
                pad.state.buttons.set(b, down);
                true
            }
            None => false,
        }
    }

    fn send_state(&mut self) {
        let merged = self
            .pads
            .values()
            .map(|p| p.state)
            .reduce(|a, b| a.merge(&b))
            .unwrap_or_default();
        if merged != self.sent {
            if let Err(e) = self.session.set_controller_state(&merged) {
                tracing::debug!("set_controller_state: {e}");
            }
            self.sent = merged;
        }
    }

    fn feedback(&mut self, f: Feedback) {
        match f {
            Feedback::Rumble { low, high } => {
                let low = (low as f32 * self.rumble_scale) as u16;
                let high = (high as f32 * self.rumble_scale) as u16;
                for pad in self.pads.values_mut() {
                    let _ = pad.gamepad.set_rumble(low, high, RUMBLE_MS);
                }
            }
            Feedback::TriggerEffects {
                type_left,
                left,
                type_right,
                right,
            } => {
                let Some(trigger) = self.trigger_intensity else {
                    return;
                };
                let report = trigger_report(
                    trigger | self.rumble_intensity.unwrap_or(0x0f),
                    type_left,
                    &left,
                    type_right,
                    &right,
                );
                for pad in self.pads.values_mut().filter(|p| p.is_dualsense) {
                    let _ = pad.gamepad.send_effect(&report);
                }
            }
            Feedback::Led(rgb) => {
                self.led = Some(rgb);
                for pad in self.pads.values_mut() {
                    let _ = pad.gamepad.set_led(rgb[0], rgb[1], rgb[2]);
                }
            }
            Feedback::PlayerIndex(i) => {
                self.player_index = Some(i);
                for pad in self.pads.values_mut() {
                    let _ = pad.gamepad.set_player_index(i as u16);
                }
            }
            Feedback::MotionReset => {
                for pad in self.pads.values_mut() {
                    pad.motion.reset();
                }
            }
            // chiaki-ng's mapping of the console's strength settings.
            Feedback::HapticIntensity(i) => {
                (self.rumble_scale, self.rumble_intensity) = match i {
                    EffectIntensity::Off => (0.0, None),
                    EffectIntensity::Weak => (0.33, Some(0x03)),
                    EffectIntensity::Medium => (0.5, Some(0x02)),
                    EffectIntensity::Strong => (1.0, Some(0x00)),
                };
            }
            Feedback::TriggerIntensity(i) => {
                self.trigger_intensity = match i {
                    EffectIntensity::Off => None,
                    EffectIntensity::Weak => Some(0x90),
                    EffectIntensity::Medium => Some(0x60),
                    EffectIntensity::Strong => Some(0x00),
                };
            }
        }
    }

    /// Stops rumble and releases the triggers when the session ends.
    fn reset_feedback(&mut self) {
        let off = trigger_report(0x00, 0x05, &[0; 10], 0x05, &[0; 10]);
        for pad in self.pads.values_mut() {
            let _ = pad.gamepad.set_rumble(0, 0, 0);
            if pad.is_dualsense {
                let _ = pad.gamepad.send_effect(&off);
            }
        }
    }
}

/// SDL's positional buttons onto the console's (south = cross, ...).
fn map_button(button: Button) -> Option<Buttons> {
    Some(match button {
        Button::South => Buttons::CROSS,
        Button::East => Buttons::CIRCLE,
        Button::West => Buttons::SQUARE,
        Button::North => Buttons::TRIANGLE,
        Button::DPadLeft => Buttons::DPAD_LEFT,
        Button::DPadRight => Buttons::DPAD_RIGHT,
        Button::DPadUp => Buttons::DPAD_UP,
        Button::DPadDown => Buttons::DPAD_DOWN,
        Button::LeftShoulder => Buttons::L1,
        Button::RightShoulder => Buttons::R1,
        Button::LeftStick => Buttons::L3,
        Button::RightStick => Buttons::R3,
        Button::Start => Buttons::OPTIONS,
        Button::Back => Buttons::SHARE,
        Button::Guide => Buttons::HOME,
        Button::Touchpad => Buttons::TOUCHPAD,
        _ => return None,
    })
}

fn apply_axis(state: &mut ControllerState, axis: Axis, value: i16) {
    match axis {
        Axis::LeftX => state.left_x = value,
        Axis::LeftY => state.left_y = value,
        Axis::RightX => state.right_x = value,
        Axis::RightY => state.right_y = value,
        // SDL triggers are 0..=32767.
        Axis::TriggerLeft => state.l2 = trigger_value(value),
        Axis::TriggerRight => state.r2 = trigger_value(value),
    }
}

fn trigger_value(value: i16) -> u8 {
    (value.max(0) >> 7) as u8
}

/// SDL's normalised touch position in the console's touchpad coordinates.
fn touch_pos(x: f32, y: f32) -> (u16, u16) {
    (
        (x.clamp(0.0, 1.0) * TOUCHPAD_WIDTH as f32) as u16,
        (y.clamp(0.0, 1.0) * TOUCHPAD_HEIGHT as f32) as u16,
    )
}

/// A DualSense effects report (the layout SDL forwards as is) that sets
/// both adaptive triggers.
fn trigger_report(
    intensity: u8,
    type_left: u8,
    left: &[u8; 10],
    type_right: u8,
    right: &[u8; 10],
) -> [u8; 47] {
    let mut r = [0u8; 47];
    r[0] = 0x04 | 0x08; // enable: right trigger, left trigger
    r[1] = 0x40; // enable: effect intensity
    r[10] = type_right;
    r[11..21].copy_from_slice(right);
    r[21] = type_left;
    r[22..32].copy_from_slice(left);
    r[36] = intensity;
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rumble_scales_to_sdl_range() {
        let f = Feedback::from_event(&rp_core::SessionEvent::Rumble {
            left: 255,
            right: 1,
        });
        assert_eq!(
            f,
            Some(Feedback::Rumble {
                low: 0xff00,
                high: 0x0100
            })
        );
        assert_eq!(
            Feedback::from_event(&rp_core::SessionEvent::Connected),
            None
        );
    }

    #[test]
    fn axes_and_triggers() {
        let mut s = ControllerState::default();
        apply_axis(&mut s, Axis::LeftY, -32768);
        apply_axis(&mut s, Axis::TriggerRight, 32767);
        apply_axis(&mut s, Axis::TriggerLeft, 0);
        assert_eq!((s.left_y, s.r2, s.l2), (-32768, 255, 0));
    }

    #[test]
    fn buttons_follow_position_not_label() {
        assert_eq!(map_button(Button::South), Some(Buttons::CROSS));
        assert_eq!(map_button(Button::East), Some(Buttons::CIRCLE));
        assert_eq!(map_button(Button::Guide), Some(Buttons::HOME));
        assert_eq!(map_button(Button::Misc1), None);
    }

    #[test]
    fn touch_coordinates() {
        assert_eq!(touch_pos(0.0, 0.0), (0, 0));
        assert_eq!(touch_pos(1.0, 1.0), (TOUCHPAD_WIDTH, TOUCHPAD_HEIGHT));
        assert_eq!(touch_pos(1.5, -1.0), (TOUCHPAD_WIDTH, 0));
    }

    #[test]
    fn trigger_report_layout() {
        let r = trigger_report(0x60, 0x21, &[1; 10], 0x26, &[2; 10]);
        assert_eq!((r[0], r[1], r[36]), (0x0c, 0x40, 0x60));
        assert_eq!((r[10], r[11], r[20]), (0x26, 2, 2));
        assert_eq!((r[21], r[22], r[31]), (0x21, 1, 1));
    }
}
