//! SDL → petal-ui translation. The input semantics (edge/level split, drag,
//! click count, canonical key names) live in `petal_ui::input::InputState`;
//! this module only maps SDL's vocabulary onto the standard one.

use sdl2::keyboard::{Mod, Scancode};
use sdl2::mouse::MouseButton;

pub use petal_ui::input::{InputEvent, InputState, Modifiers};

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
