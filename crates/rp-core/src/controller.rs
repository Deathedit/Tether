//! Controller input for a session: the state chiaki sends to the console
//! (buttons, sticks, triggers, touchpad, motion), plus a motion tracker that
//! turns gyro and accelerometer readings into the orientation the console
//! expects.

use std::ops::{BitOr, BitOrAssign};

use chiaki_sys as sys;

/// Controller buttons, as a bit set. Names follow the console's layout:
/// `CROSS` is the bottom face button, `CIRCLE` the right one, and so on.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Buttons(pub u32);

impl Buttons {
    pub const NONE: Buttons = Buttons(0);
    pub const CROSS: Buttons = Buttons(1 << 0);
    pub const CIRCLE: Buttons = Buttons(1 << 1);
    pub const SQUARE: Buttons = Buttons(1 << 2);
    pub const TRIANGLE: Buttons = Buttons(1 << 3);
    pub const DPAD_LEFT: Buttons = Buttons(1 << 4);
    pub const DPAD_RIGHT: Buttons = Buttons(1 << 5);
    pub const DPAD_UP: Buttons = Buttons(1 << 6);
    pub const DPAD_DOWN: Buttons = Buttons(1 << 7);
    pub const L1: Buttons = Buttons(1 << 8);
    pub const R1: Buttons = Buttons(1 << 9);
    pub const L3: Buttons = Buttons(1 << 10);
    pub const R3: Buttons = Buttons(1 << 11);
    pub const OPTIONS: Buttons = Buttons(1 << 12);
    /// "Create" on a DualSense.
    pub const SHARE: Buttons = Buttons(1 << 13);
    /// Clicking the touchpad.
    pub const TOUCHPAD: Buttons = Buttons(1 << 14);
    /// The home button in the middle.
    pub const HOME: Buttons = Buttons(1 << 15);

    pub fn contains(self, other: Buttons) -> bool {
        self.0 & other.0 == other.0
    }

    pub fn set(&mut self, buttons: Buttons, pressed: bool) {
        if pressed {
            self.0 |= buttons.0;
        } else {
            self.0 &= !buttons.0;
        }
    }
}

impl BitOr for Buttons {
    type Output = Buttons;
    fn bitor(self, rhs: Buttons) -> Buttons {
        Buttons(self.0 | rhs.0)
    }
}

impl BitOrAssign for Buttons {
    fn bitor_assign(&mut self, rhs: Buttons) {
        self.0 |= rhs.0;
    }
}

/// Touchpad size in the console's coordinates.
pub const TOUCHPAD_WIDTH: u16 = 1920;
pub const TOUCHPAD_HEIGHT: u16 = 1079;

/// Fingers the touchpad tracks at once.
pub const MAX_TOUCHES: usize = 2;

/// One finger on the touchpad.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Touch {
    /// `None` when this slot has no finger.
    pub id: Option<u8>,
    pub x: u16,
    pub y: u16,
}

impl Touch {
    const UP: Touch = Touch {
        id: None,
        x: 0,
        y: 0,
    };
}

/// Touch ids wrap at 7 bits, like on the real controller.
const TOUCH_ID_MASK: u8 = 0x7f;

/// Everything a controller reports to the console.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ControllerState {
    pub buttons: Buttons,
    /// Analog triggers, 0 (released) to 255 (fully pressed).
    pub l2: u8,
    pub r2: u8,
    /// Sticks, -32768..=32767; +x is right, +y is down.
    pub left_x: i16,
    pub left_y: i16,
    pub right_x: i16,
    pub right_y: i16,
    pub touches: [Touch; MAX_TOUCHES],
    touch_id_next: u8,
    /// Angular velocity, rad/s.
    pub gyro: [f32; 3],
    /// Acceleration in g. At rest and flat this is (0, 1, 0).
    pub accel: [f32; 3],
    /// Orientation quaternion (x, y, z, w); see [`MotionTracker`].
    pub orient: [f32; 4],
}

impl Default for ControllerState {
    /// Nothing pressed, sticks centred, held flat and still.
    fn default() -> Self {
        Self {
            buttons: Buttons::NONE,
            l2: 0,
            r2: 0,
            left_x: 0,
            left_y: 0,
            right_x: 0,
            right_y: 0,
            touches: [Touch::UP; MAX_TOUCHES],
            touch_id_next: 0,
            gyro: [0.0; 3],
            accel: [0.0, 1.0, 0.0],
            orient: [0.0, 0.0, 0.0, 1.0],
        }
    }
}

impl ControllerState {
    /// Puts a finger down. Returns its id, or `None` if both slots are taken.
    pub fn start_touch(&mut self, x: u16, y: u16) -> Option<u8> {
        let slot = self.touches.iter_mut().find(|t| t.id.is_none())?;
        let id = self.touch_id_next;
        self.touch_id_next = (self.touch_id_next + 1) & TOUCH_ID_MASK;
        *slot = Touch { id: Some(id), x, y };
        Some(id)
    }

    pub fn move_touch(&mut self, id: u8, x: u16, y: u16) {
        if let Some(t) = self.touches.iter_mut().find(|t| t.id == Some(id)) {
            t.x = x;
            t.y = y;
        }
    }

    pub fn stop_touch(&mut self, id: u8) {
        if let Some(t) = self.touches.iter_mut().find(|t| t.id == Some(id)) {
            *t = Touch::UP;
        }
    }

    /// Whether gyro or accelerometer differ from the at-rest defaults.
    fn has_motion(&self) -> bool {
        let rest = ControllerState::default();
        self.gyro != rest.gyro || self.accel != rest.accel
    }

    /// Combines two controllers into one, so either can drive the console:
    /// buttons are OR-ed, triggers and sticks take the stronger input,
    /// touches the first controller with a finger in each slot, and motion
    /// comes whole from one controller (mixing two would make nonsense).
    pub fn merge(&self, other: &ControllerState) -> ControllerState {
        fn max_abs(a: i16, b: i16) -> i16 {
            if a.unsigned_abs() >= b.unsigned_abs() {
                a
            } else {
                b
            }
        }
        let motion = if self.has_motion() || !other.has_motion() {
            self
        } else {
            other
        };
        let mut touches = [Touch::UP; MAX_TOUCHES];
        for (i, t) in touches.iter_mut().enumerate() {
            if self.touches[i].id.is_some() {
                *t = self.touches[i];
            } else if other.touches[i].id.is_some() {
                *t = other.touches[i];
            }
        }
        ControllerState {
            buttons: self.buttons | other.buttons,
            l2: self.l2.max(other.l2),
            r2: self.r2.max(other.r2),
            left_x: max_abs(self.left_x, other.left_x),
            left_y: max_abs(self.left_y, other.left_y),
            right_x: max_abs(self.right_x, other.right_x),
            right_y: max_abs(self.right_y, other.right_y),
            touches,
            touch_id_next: 0,
            gyro: motion.gyro,
            accel: motion.accel,
            orient: motion.orient,
        }
    }

    pub(crate) fn to_sys(self) -> sys::ChiakiControllerState {
        let mut touches = [sys::ChiakiControllerTouch::default(); MAX_TOUCHES];
        for (dst, src) in touches.iter_mut().zip(self.touches.iter()) {
            *dst = sys::ChiakiControllerTouch {
                x: src.x,
                y: src.y,
                id: src.id.map_or(-1, |id| (id & TOUCH_ID_MASK) as i8),
            };
        }
        sys::ChiakiControllerState {
            buttons: self.buttons.0,
            l2_state: self.l2,
            r2_state: self.r2,
            left_x: self.left_x,
            left_y: self.left_y,
            right_x: self.right_x,
            right_y: self.right_y,
            touch_id_next: self.touch_id_next,
            touches,
            gyro_x: self.gyro[0],
            gyro_y: self.gyro[1],
            gyro_z: self.gyro[2],
            accel_x: self.accel[0],
            accel_y: self.accel[1],
            accel_z: self.accel[2],
            orient_x: self.orient[0],
            orient_y: self.orient[1],
            orient_z: self.orient[2],
            orient_w: self.orient[3],
        }
    }
}

/// Turns gyro and accelerometer readings into the orientation quaternion the
/// console expects (chiaki's Madgwick filter). Feed it every sensor reading
/// and copy the result into the state with [`MotionTracker::apply`].
pub struct MotionTracker {
    tracker: sys::ChiakiOrientationTracker,
    /// The accelerometer reading treated as "level" (set by `reset`).
    accel_zero: sys::ChiakiAccelNewZero,
    /// The latest raw accelerometer reading.
    real_accel: sys::ChiakiAccelNewZero,
}

impl Default for MotionTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl MotionTracker {
    pub fn new() -> Self {
        let mut t = Self {
            tracker: Default::default(),
            accel_zero: Default::default(),
            real_accel: Default::default(),
        };
        // SAFETY: plain structs, initialised in place.
        unsafe {
            sys::chiaki_orientation_tracker_init(&mut t.tracker);
            sys::chiaki_accel_new_zero_set_inactive(&mut t.accel_zero, false);
            sys::chiaki_accel_new_zero_set_inactive(&mut t.real_accel, true);
        }
        t
    }

    /// A gyroscope reading in rad/s. `timestamp_us` is the sensor's
    /// timestamp in microseconds (wrapping is fine).
    pub fn update_gyro(&mut self, gyro: [f32; 3], timestamp_us: u32) {
        let [ax, ay, az] = [
            self.tracker.accel_x,
            self.tracker.accel_y,
            self.tracker.accel_z,
        ];
        // SAFETY: valid tracker and accel_zero.
        unsafe {
            sys::chiaki_orientation_tracker_update(
                &mut self.tracker,
                gyro[0],
                gyro[1],
                gyro[2],
                ax,
                ay,
                az,
                &mut self.accel_zero,
                true,
                timestamp_us,
            )
        };
    }

    /// An accelerometer reading in g (m/s² divided by standard gravity).
    pub fn update_accel(&mut self, accel: [f32; 3], timestamp_us: u32) {
        let [gx, gy, gz] = [
            self.tracker.gyro_x,
            self.tracker.gyro_y,
            self.tracker.gyro_z,
        ];
        // SAFETY: valid structs.
        unsafe {
            sys::chiaki_accel_new_zero_set_active(
                &mut self.real_accel,
                accel[0],
                accel[1],
                accel[2],
                true,
            );
            sys::chiaki_orientation_tracker_update(
                &mut self.tracker,
                gx,
                gy,
                gz,
                accel[0],
                accel[1],
                accel[2],
                &mut self.accel_zero,
                false,
                timestamp_us,
            )
        };
    }

    /// Copies gyro, accelerometer and orientation into `state`.
    pub fn apply(&mut self, state: &mut ControllerState) {
        let mut raw = state.to_sys();
        // SAFETY: valid tracker and state.
        unsafe {
            sys::chiaki_orientation_tracker_apply_to_controller_state(&mut self.tracker, &mut raw)
        };
        state.gyro = [raw.gyro_x, raw.gyro_y, raw.gyro_z];
        state.accel = [raw.accel_x, raw.accel_y, raw.accel_z];
        state.orient = [raw.orient_x, raw.orient_y, raw.orient_z, raw.orient_w];
    }

    /// Treats the controller's current position as level (the console asks
    /// for this with `SessionEvent::MotionReset`).
    pub fn reset(&mut self) {
        let (ax, ay, az) = (
            self.real_accel.accel_x,
            self.real_accel.accel_y,
            self.real_accel.accel_z,
        );
        let timestamp = self.tracker.timestamp;
        // SAFETY: valid structs.
        unsafe {
            sys::chiaki_accel_new_zero_set_active(&mut self.accel_zero, ax, ay, az, false);
            sys::chiaki_orientation_tracker_init(&mut self.tracker);
            sys::chiaki_orientation_tracker_update(
                &mut self.tracker,
                0.0,
                0.0,
                0.0,
                ax,
                ay,
                az,
                &mut self.accel_zero,
                false,
                timestamp,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_matches_chiaki_idle() {
        let mut idle = sys::ChiakiControllerState::default();
        // SAFETY: plain struct.
        unsafe { sys::chiaki_controller_state_set_idle(&mut idle) };
        let ours = ControllerState::default().to_sys();
        // SAFETY: both valid.
        assert!(unsafe { sys::chiaki_controller_state_equals(&mut idle, &mut ours.clone()) });
        assert_eq!(ours.touches[0].id, -1);
    }

    #[test]
    #[allow(clippy::unnecessary_cast)] // C enums are i32 on MSVC, u32 elsewhere
    fn buttons() {
        let mut b = Buttons::NONE;
        b.set(Buttons::CROSS, true);
        b |= Buttons::L1;
        assert!(b.contains(Buttons::CROSS | Buttons::L1));
        b.set(Buttons::CROSS, false);
        assert!(!b.contains(Buttons::CROSS));
        assert_eq!(Buttons::HOME.0, sys::CHIAKI_CONTROLLER_BUTTON_PS as u32);
        assert_eq!(Buttons::CIRCLE.0, sys::CHIAKI_CONTROLLER_BUTTON_MOON as u32);
        assert_eq!(
            Buttons::TRIANGLE.0,
            sys::CHIAKI_CONTROLLER_BUTTON_PYRAMID as u32
        );
    }

    #[test]
    fn touches() {
        let mut s = ControllerState::default();
        let a = s.start_touch(10, 20).unwrap();
        let b = s.start_touch(30, 40).unwrap();
        assert_ne!(a, b);
        assert_eq!(s.start_touch(0, 0), None, "only two slots");
        s.move_touch(a, 11, 21);
        assert_eq!(
            s.touches[0],
            Touch {
                id: Some(a),
                x: 11,
                y: 21
            }
        );
        s.stop_touch(a);
        assert_eq!(s.touches[0].id, None);
        let c = s.start_touch(5, 5).unwrap();
        assert!(c != a && c != b, "ids keep counting up");
        assert_eq!(s.to_sys().touches[1].id, b as i8);
    }

    #[test]
    fn touch_ids_wrap_at_7_bits() {
        let mut s = ControllerState::default();
        for _ in 0..200 {
            let id = s.start_touch(0, 0).unwrap();
            assert!(id <= TOUCH_ID_MASK);
            s.stop_touch(id);
        }
    }

    #[test]
    fn merge_two_controllers() {
        let mut a = ControllerState::default();
        let mut b = ControllerState::default();
        a.buttons = Buttons::CROSS;
        a.left_x = -100;
        a.l2 = 10;
        b.buttons = Buttons::CIRCLE;
        b.left_x = 50;
        b.left_y = 3000;
        b.l2 = 200;
        b.gyro = [1.0, 0.0, 0.0];
        b.start_touch(7, 8);
        let m = a.merge(&b);
        assert_eq!(m.buttons, Buttons::CROSS | Buttons::CIRCLE);
        assert_eq!((m.left_x, m.left_y, m.l2), (-100, 3000, 200));
        assert_eq!(m.gyro, [1.0, 0.0, 0.0], "motion from the one that moves");
        assert_eq!(m.touches[0].x, 7);
        // Same result as chiaki's own merge for the non-motion fields.
        let (mut ra, mut rb, mut out) = (
            a.to_sys(),
            b.to_sys(),
            sys::ChiakiControllerState::default(),
        );
        // SAFETY: valid structs.
        unsafe { sys::chiaki_controller_state_or(&mut out, &mut ra, &mut rb) };
        assert_eq!(out.buttons, m.buttons.0);
        assert_eq!((out.left_x, out.left_y, out.l2_state), (-100, 3000, 200));
        assert_eq!(out.gyro_x, 1.0);
    }

    #[test]
    fn motion_tracker_at_rest_stays_level() {
        let mut t = MotionTracker::new();
        let mut s = ControllerState::default();
        for i in 0..100u32 {
            t.update_accel([0.0, 1.0, 0.0], i * 4000);
            t.update_gyro([0.0, 0.0, 0.0], i * 4000 + 2000);
        }
        t.apply(&mut s);
        assert!((s.accel[1] - 1.0).abs() < 0.01, "{:?}", s.accel);
        let w = s.orient[3].abs();
        assert!(
            w > 0.99,
            "orientation should stay near identity: {:?}",
            s.orient
        );
        t.reset();
    }
}
