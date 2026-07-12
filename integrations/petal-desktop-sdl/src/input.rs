//! SDL → petal-ui translation. The input semantics (edge/level split, drag,
//! click count, canonical key names) live in `petal_ui::input::InputState`;
//! this module only maps SDL's vocabulary onto the standard one and pumps the
//! event queue into it.

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
pub fn poll_sdl_events(event_pump: &mut sdl2::EventPump, input: &mut InputState) -> PollResult {
    let mut result = PollResult::None;
    for event in event_pump.poll_iter() {
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
