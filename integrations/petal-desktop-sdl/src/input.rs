//! SDL → petal-ui translation. The input semantics (edge/level split, drag,
//! click count, canonical key names) live in `petal_ui::input::InputState`;
//! this module only maps SDL's vocabulary onto the standard one and pumps the
//! event queue into it.
//!
//! Gamepads ride the *same* normalized key stream as the keyboard (see
//! [`Gamepads`]): there is no separate pad vocabulary, so every script and
//! every host that already reads `key_down("left")` gets controller support for
//! free, and a game's keyboard and pad bindings can never drift apart.

use sdl2::controller::{Axis, Button, GameController};
use sdl2::event::Event;
use sdl2::keyboard::{Mod, Scancode};
use sdl2::mouse::MouseButton;

pub use petal_ui::input::{InputEvent, InputState, Modifiers};

/// Result of pumping the SDL event queue for one iteration of the loop.
pub enum PollResult {
    None,
    Quit,
    Escape,
}

/// Drain the SDL event queue into `input` as normalized `petal_ui` events, and
/// report window-level signals (quit / escape) back to the loop. The caller
/// starts the script frame afterwards with `input.begin_frame(dt)`.
///
/// Keyboard and mouse only. Hosts that want controllers call
/// [`poll_sdl_events_with_gamepads`] instead, which needs somewhere to keep the
/// open-device and stick-latch state between frames.
pub fn poll_sdl_events(event_pump: &mut sdl2::EventPump, input: &mut InputState) -> PollResult {
    poll_sdl_events_with_gamepads(event_pump, input, &mut Gamepads::disabled())
}

/// As [`poll_sdl_events`], but also opens/closes game controllers as they are
/// hot-plugged and folds their buttons and left stick into the same key stream.
pub fn poll_sdl_events_with_gamepads(
    event_pump: &mut sdl2::EventPump,
    input: &mut InputState,
    gamepads: &mut Gamepads,
) -> PollResult {
    let mut result = PollResult::None;
    for event in event_pump.poll_iter() {
        if gamepads.handle_event(&event, input) {
            continue;
        }
        match event {
            Event::Quit { .. } => return PollResult::Quit,
            Event::KeyDown {
                scancode: Some(sc), ..
            } if sc == Scancode::Escape => {
                result = PollResult::Escape;
            }
            // OS auto-repeats are dropped: `key_pressed` fires once per
            // physical press, matching the pre-petal-ui behavior.
            Event::KeyDown {
                scancode: Some(sc),
                keymod,
                repeat: false,
                ..
            } => {
                input.event(InputEvent::Modifiers(mods_from_sdl(keymod)));
                if let Some(name) = scancode_to_name(sc) {
                    input.event(InputEvent::KeyDown {
                        key: name.to_string(),
                    });
                }
            }
            Event::KeyUp {
                scancode: Some(sc),
                keymod,
                ..
            } => {
                input.event(InputEvent::Modifiers(mods_from_sdl(keymod)));
                if let Some(name) = scancode_to_name(sc) {
                    input.event(InputEvent::KeyUp {
                        key: name.to_string(),
                    });
                }
            }
            Event::TextInput { text, .. } => {
                input.event(InputEvent::Text { text });
            }
            Event::MouseMotion {
                x, y, xrel, yrel, ..
            } => {
                // Both the absolute position and the raw delta: the delta keeps
                // flowing (mouselook) even while the pointer is grabbed/locked
                // and the absolute position stops moving.
                input.event(InputEvent::MouseMove { x, y });
                input.event(InputEvent::MouseRelative { dx: xrel, dy: yrel });
            }
            Event::MouseButtonDown { mouse_btn, .. } => {
                if let Some(button) = sdl_button_to_std(mouse_btn) {
                    input.event(InputEvent::MouseDown { button });
                }
            }
            Event::MouseButtonUp { mouse_btn, .. } => {
                if let Some(button) = sdl_button_to_std(mouse_btn) {
                    input.event(InputEvent::MouseUp { button });
                }
            }
            Event::MouseWheel {
                precise_x,
                precise_y,
                ..
            } => {
                // SDL y > 0 means "scrolled up"; the standard scroll_y() is
                // positive scrolling down.
                input.event(InputEvent::Scroll {
                    dx: precise_x as f64,
                    dy: -precise_y as f64,
                });
            }
            _ => {}
        }
    }
    result
}

/// Map an SDL scancode to the canonical petal-ui key name.
pub fn scancode_to_name(code: Scancode) -> Option<&'static str> {
    Some(match code {
        Scancode::A => "a",
        Scancode::B => "b",
        Scancode::C => "c",
        Scancode::D => "d",
        Scancode::E => "e",
        Scancode::F => "f",
        Scancode::G => "g",
        Scancode::H => "h",
        Scancode::I => "i",
        Scancode::J => "j",
        Scancode::K => "k",
        Scancode::L => "l",
        Scancode::M => "m",
        Scancode::N => "n",
        Scancode::O => "o",
        Scancode::P => "p",
        Scancode::Q => "q",
        Scancode::R => "r",
        Scancode::S => "s",
        Scancode::T => "t",
        Scancode::U => "u",
        Scancode::V => "v",
        Scancode::W => "w",
        Scancode::X => "x",
        Scancode::Y => "y",
        Scancode::Z => "z",
        Scancode::Num0 => "0",
        Scancode::Num1 => "1",
        Scancode::Num2 => "2",
        Scancode::Num3 => "3",
        Scancode::Num4 => "4",
        Scancode::Num5 => "5",
        Scancode::Num6 => "6",
        Scancode::Num7 => "7",
        Scancode::Num8 => "8",
        Scancode::Num9 => "9",
        Scancode::Return => "return",
        Scancode::Escape => "escape",
        Scancode::Backspace => "backspace",
        Scancode::Tab => "tab",
        Scancode::Space => "space",
        Scancode::Up => "up",
        Scancode::Down => "down",
        Scancode::Left => "left",
        Scancode::Right => "right",
        Scancode::PageUp => "pageup",
        Scancode::PageDown => "pagedown",
        Scancode::Home => "home",
        Scancode::End => "end",
        Scancode::Delete => "delete",
        Scancode::Insert => "insert",
        Scancode::LShift | Scancode::RShift => "shift",
        Scancode::LCtrl | Scancode::RCtrl => "ctrl",
        Scancode::LAlt | Scancode::RAlt => "alt",
        Scancode::LGui | Scancode::RGui => "cmd",
        Scancode::Minus => "minus",
        Scancode::Equals => "equals",
        Scancode::Comma => "comma",
        Scancode::Period => "period",
        Scancode::Slash => "slash",
        Scancode::Backslash => "backslash",
        Scancode::Semicolon => "semicolon",
        Scancode::Apostrophe => "quote",
        Scancode::Grave => "backquote",
        Scancode::LeftBracket => "leftbracket",
        Scancode::RightBracket => "rightbracket",
        Scancode::F1 => "f1",
        Scancode::F2 => "f2",
        Scancode::F3 => "f3",
        Scancode::F4 => "f4",
        Scancode::F5 => "f5",
        Scancode::F6 => "f6",
        Scancode::F7 => "f7",
        Scancode::F8 => "f8",
        Scancode::F9 => "f9",
        Scancode::F10 => "f10",
        Scancode::F11 => "f11",
        Scancode::F12 => "f12",
        _ => return None,
    })
}

/// Map an SDL mouse button to the standard petal-ui id (0 = left, 1 = right,
/// 2 = middle).
pub fn sdl_button_to_std(btn: MouseButton) -> Option<u8> {
    Some(match btn {
        MouseButton::Left => petal_ui::input::buttons::LEFT,
        MouseButton::Right => petal_ui::input::buttons::RIGHT,
        MouseButton::Middle => petal_ui::input::buttons::MIDDLE,
        _ => return None,
    })
}

/// Translate an SDL modifier chord into the standard modifier record.
pub fn mods_from_sdl(m: Mod) -> Modifiers {
    Modifiers {
        shift: m.intersects(Mod::LSHIFTMOD | Mod::RSHIFTMOD),
        ctrl: m.intersects(Mod::LCTRLMOD | Mod::RCTRLMOD),
        alt: m.intersects(Mod::LALTMOD | Mod::RALTMOD),
        cmd: m.intersects(Mod::LGUIMOD | Mod::RGUIMOD),
    }
}

// --- Game controllers ---
//
// A pad is not a new input device as far as scripts are concerned: it is a
// second way to press the keys the game already binds. That keeps the whole
// feature inside this module — nothing downstream (petal-ui, the agent
// protocol, the natives, the scripts) learns a new vocabulary — and it means a
// cart written for the keyboard is playable on a controller without touching
// the cart.
//
// The mapping below is the crate's convention. Pad slots are assigned in
// connection order and stay stable across a disconnect (slot 0 stays slot 0
// while its controller is unplugged, so pulling player 2's cable does not
// promote them to player 1).
//
//   slot 0                              slot 1
//     d-pad / left stick -> up down left right      -> i k j l
//     south (A)          -> z                       -> n
//     east  (B)          -> x                       -> m
//     west  (X)          -> c                       -> comma
//     north (Y)          -> v                       -> period
//     start              -> return                  -> (unmapped)
//     back / select      -> shift                   -> (unmapped)
//     left shoulder      -> leftbracket             -> (unmapped)
//     right shoulder     -> rightbracket            -> (unmapped)
//
// Slot 0's letters are the same ones the "NES-style" convention uses (Z/X for
// A/B, Enter for start, RShift for select), and its directions are the arrow
// keys, so the common case — a game that reads arrows plus z/x — works with no
// per-host configuration. Slots 2+ are ignored rather than mapped onto
// arbitrary keys.
//
// The left stick is quantized into the same four direction keys as the d-pad,
// with hysteresis (a wide press threshold, a narrower release threshold) so a
// stick resting near the boundary does not chatter key events every frame.

/// Left-stick magnitude (of 32767) at which a direction starts counting as
/// held, and the lower value at which it stops. The gap is deliberate.
const AXIS_PRESS: i16 = 16384;
const AXIS_RELEASE: i16 = 10240;

/// The per-slot key names, indexed by [`PadKey`]. `None` means "this pad has no
/// equivalent for that button" and the event is dropped.
type PadMap = [Option<&'static str>; 9];

const PAD_MAPS: [PadMap; 2] = [
    [
        Some("up"),
        Some("down"),
        Some("left"),
        Some("right"),
        Some("z"),
        Some("x"),
        Some("c"),
        Some("v"),
        Some("return"),
    ],
    [
        Some("i"),
        Some("k"),
        Some("j"),
        Some("l"),
        Some("n"),
        Some("m"),
        Some("comma"),
        Some("period"),
        None,
    ],
];

/// Extra slot-0-only keys that don't fit the shared table above.
fn extra_key(slot: usize, button: Button) -> Option<&'static str> {
    if slot != 0 {
        return None;
    }
    Some(match button {
        Button::Back => "shift",
        Button::LeftShoulder => "leftbracket",
        Button::RightShoulder => "rightbracket",
        _ => return None,
    })
}

/// Indices into [`PadMap`]. Also the identity of a latched stick direction.
const K_UP: usize = 0;
const K_DOWN: usize = 1;
const K_LEFT: usize = 2;
const K_RIGHT: usize = 3;
const K_SOUTH: usize = 4;
const K_EAST: usize = 5;
const K_WEST: usize = 6;
const K_NORTH: usize = 7;
const K_START: usize = 8;

fn button_index(button: Button) -> Option<usize> {
    Some(match button {
        Button::DPadUp => K_UP,
        Button::DPadDown => K_DOWN,
        Button::DPadLeft => K_LEFT,
        Button::DPadRight => K_RIGHT,
        Button::A => K_SOUTH,
        Button::B => K_EAST,
        Button::X => K_WEST,
        Button::Y => K_NORTH,
        Button::Start => K_START,
        _ => return None,
    })
}

/// One connected controller and the stick directions it currently holds.
struct Pad {
    controller: GameController,
    instance_id: u32,
    /// Latched left-stick directions, indexed by `K_UP`..=`K_RIGHT`.
    stick: [bool; 4],
}

/// Open game controllers and the state needed to turn their analog input into
/// discrete key events. Owned by the run loop and passed to
/// [`poll_sdl_events_with_gamepads`] each frame.
///
/// Construction never fails in a way that matters: if the controller subsystem
/// is unavailable this is simply inert, and a host that never sees a controller
/// behaves exactly as it did before gamepads existed.
pub struct Gamepads {
    subsystem: Option<sdl2::GameControllerSubsystem>,
    /// Indexed by pad slot; a `None` slot is a pad that has been unplugged and
    /// is reserved for the next controller to take its place.
    slots: Vec<Option<Pad>>,
}

impl Gamepads {
    /// Open the controller subsystem. Any failure (no subsystem, no permission)
    /// degrades to [`Gamepads::disabled`] with a one-line note, because losing
    /// controller support is never a reason to fail to start.
    pub fn new(sdl: &sdl2::Sdl) -> Self {
        match sdl.game_controller() {
            Ok(subsystem) => {
                // Controllers already plugged in at startup don't generate an
                // "added" event, so enumerate once here.
                let mut pads = Self {
                    subsystem: Some(subsystem),
                    slots: Vec::new(),
                };
                pads.open_present_devices();
                pads
            }
            Err(e) => {
                eprintln!("[gamepad] unavailable: {}", e);
                Self::disabled()
            }
        }
    }

    /// An inert instance: accepts events, opens nothing, emits nothing.
    pub fn disabled() -> Self {
        Self {
            subsystem: None,
            slots: Vec::new(),
        }
    }

    /// How many controllers are currently connected.
    pub fn connected_count(&self) -> usize {
        self.slots.iter().filter(|s| s.is_some()).count()
    }

    fn open_present_devices(&mut self) {
        let Some(subsystem) = &self.subsystem else {
            return;
        };
        let present: Vec<u32> = (0..subsystem.num_joysticks().unwrap_or(0))
            .filter(|i| subsystem.is_game_controller(*i))
            .collect();
        for index in present {
            self.open_device(index);
        }
    }

    /// Open the joystick at `device_index` into the lowest free slot. Devices
    /// that aren't controllers, or that fail to open (yanked mid-enumeration),
    /// are skipped silently — hot-plug races must not panic.
    fn open_device(&mut self, device_index: u32) {
        let Some(subsystem) = &self.subsystem else {
            return;
        };
        let Ok(controller) = subsystem.open(device_index) else {
            return;
        };
        let instance_id = controller.instance_id();
        if self.slot_of(instance_id).is_some() {
            return; // Already open (duplicate add event).
        }
        let pad = Pad {
            controller,
            instance_id,
            stick: [false; 4],
        };
        match self.slots.iter().position(|s| s.is_none()) {
            Some(slot) => self.slots[slot] = Some(pad),
            None => self.slots.push(Some(pad)),
        }
    }

    fn slot_of(&self, instance_id: u32) -> Option<usize> {
        self.slots
            .iter()
            .position(|s| s.as_ref().is_some_and(|p| p.instance_id == instance_id))
    }

    /// Release a controller, and release any keys its stick was holding so a
    /// script doesn't see a direction stuck down forever after an unplug.
    fn close_device(&mut self, instance_id: u32, input: &mut InputState) {
        let Some(slot) = self.slot_of(instance_id) else {
            return;
        };
        if let Some(pad) = self.slots[slot].take() {
            for (dir, held) in pad.stick.iter().enumerate() {
                if *held {
                    emit(input, slot, dir, false);
                }
            }
            drop(pad.controller);
        }
    }

    /// Translate one SDL event. Returns `true` when the event was a controller
    /// event and has been consumed, so the caller's keyboard/mouse match can
    /// skip it.
    fn handle_event(&mut self, event: &Event, input: &mut InputState) -> bool {
        match *event {
            Event::ControllerDeviceAdded { which, .. } => {
                self.open_device(which);
                true
            }
            Event::ControllerDeviceRemoved { which, .. } => {
                self.close_device(which, input);
                true
            }
            Event::ControllerButtonDown { which, button, .. } => {
                self.button(which, button, true, input);
                true
            }
            Event::ControllerButtonUp { which, button, .. } => {
                self.button(which, button, false, input);
                true
            }
            Event::ControllerAxisMotion {
                which, axis, value, ..
            } => {
                self.axis(which, axis, value, input);
                true
            }
            _ => false,
        }
    }

    fn button(&mut self, instance_id: u32, button: Button, down: bool, input: &mut InputState) {
        let Some(slot) = self.slot_of(instance_id) else {
            return;
        };
        if let Some(index) = button_index(button) {
            emit(input, slot, index, down);
        } else if let Some(name) = extra_key(slot, button) {
            send(input, name, down);
        }
    }

    /// Quantize a stick axis into a pair of opposed direction keys, emitting
    /// only on a latch change. Axes other than the left stick are ignored: the
    /// right stick and triggers have no sensible key equivalent, and a host
    /// that wants them analog should read the pad directly.
    fn axis(&mut self, instance_id: u32, axis: Axis, value: i16, input: &mut InputState) {
        let (negative, positive) = match axis {
            Axis::LeftX => (K_LEFT, K_RIGHT),
            Axis::LeftY => (K_UP, K_DOWN),
            _ => return,
        };
        let Some(slot) = self.slot_of(instance_id) else {
            return;
        };
        // How far the stick is pushed toward each of the two opposed
        // directions. `value` is signed with an asymmetric range
        // (-32768..=32767), so saturate before negating.
        let pushes = [
            (negative, value.saturating_neg().max(0)),
            (positive, value.max(0)),
        ];
        for (dir, push) in pushes {
            let held = self.slots[slot].as_ref().is_some_and(|p| p.stick[dir]);
            let next = if held {
                push >= AXIS_RELEASE
            } else {
                push >= AXIS_PRESS
            };
            if next != held {
                if let Some(pad) = self.slots[slot].as_mut() {
                    pad.stick[dir] = next;
                }
                emit(input, slot, dir, next);
            }
        }
    }
}

/// Emit the key a pad slot maps `index` to, if it maps one at all.
fn emit(input: &mut InputState, slot: usize, index: usize, down: bool) {
    let Some(map) = PAD_MAPS.get(slot) else {
        return;
    };
    if let Some(name) = map[index] {
        send(input, name, down);
    }
}

fn send(input: &mut InputState, name: &str, down: bool) {
    let key = name.to_string();
    input.event(if down {
        InputEvent::KeyDown { key }
    } else {
        InputEvent::KeyUp { key }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_pad_key_is_canonical() {
        for map in PAD_MAPS.iter() {
            for name in map.iter().flatten() {
                assert!(
                    petal_ui::input::is_canonical_key(name),
                    "pad key {:?} is not a canonical petal-ui key name",
                    name
                );
            }
        }
        for button in [Button::Back, Button::LeftShoulder, Button::RightShoulder] {
            let name = extra_key(0, button).expect("slot 0 maps this button");
            assert!(petal_ui::input::is_canonical_key(name));
        }
    }

    #[test]
    fn slot_1_has_no_key_in_common_with_slot_0() {
        for a in PAD_MAPS[0].iter().flatten() {
            assert!(
                !PAD_MAPS[1].iter().flatten().any(|b| b == a),
                "pads 0 and 1 both map {:?}",
                a
            );
        }
    }

    #[test]
    fn disabled_gamepads_consume_nothing() {
        let mut pads = Gamepads::disabled();
        let mut input = InputState::default();
        // A device-added event on an inert instance must not open or panic.
        let added = Event::ControllerDeviceAdded {
            timestamp: 0,
            which: 0,
        };
        assert!(pads.handle_event(&added, &mut input));
        assert_eq!(pads.connected_count(), 0);
        assert!(!input.is_key_down("z"));
    }

    #[test]
    fn events_for_unknown_instances_are_ignored() {
        let mut pads = Gamepads::disabled();
        let mut input = InputState::default();
        pads.button(99, Button::A, true, &mut input);
        pads.axis(99, Axis::LeftX, -30000, &mut input);
        pads.close_device(99, &mut input);
        assert!(!input.is_key_down("z"));
        assert!(!input.is_key_down("left"));
    }
}
