use std::collections::HashSet;

use sdl2::keyboard::Scancode;

#[derive(Default)]
pub struct InputState {
    pub keys_down: HashSet<String>,
    pub keys_pressed: HashSet<String>, // Only set for the frame a key transitions to down
    pub mouse_x: i32,
    pub mouse_y: i32,
    /// Mouse delta since last begin_frame(). Accumulated from SDL MouseMotion
    /// xrel/yrel events so that relative motion works even when the cursor is
    /// locked in the middle of the screen.
    pub mouse_dx: i32,
    pub mouse_dy: i32,
    pub mouse_buttons: HashSet<u8>,
    pub mouse_buttons_pressed: HashSet<u8>,
}

impl InputState {
    /// Clear per-frame "pressed" and relative-motion state. Called at the top
    /// of each frame *before* polling SDL events.
    pub fn begin_frame(&mut self) {
        self.keys_pressed.clear();
        self.mouse_buttons_pressed.clear();
        self.mouse_dx = 0;
        self.mouse_dy = 0;
    }
}

/// Map an SDL2 scancode to the lowercase string names used by Petal scripts.
pub fn scancode_to_name(sc: Scancode) -> Option<&'static str> {
    Some(match sc {
        Scancode::A => "a", Scancode::B => "b", Scancode::C => "c",
        Scancode::D => "d", Scancode::E => "e", Scancode::F => "f",
        Scancode::G => "g", Scancode::H => "h", Scancode::I => "i",
        Scancode::J => "j", Scancode::K => "k", Scancode::L => "l",
        Scancode::M => "m", Scancode::N => "n", Scancode::O => "o",
        Scancode::P => "p", Scancode::Q => "q", Scancode::R => "r",
        Scancode::S => "s", Scancode::T => "t", Scancode::U => "u",
        Scancode::V => "v", Scancode::W => "w", Scancode::X => "x",
        Scancode::Y => "y", Scancode::Z => "z",
        Scancode::Num0 => "0", Scancode::Num1 => "1", Scancode::Num2 => "2",
        Scancode::Num3 => "3", Scancode::Num4 => "4", Scancode::Num5 => "5",
        Scancode::Num6 => "6", Scancode::Num7 => "7", Scancode::Num8 => "8",
        Scancode::Num9 => "9",
        Scancode::Space => "space",
        Scancode::Return => "return",
        Scancode::Escape => "escape",
        Scancode::Tab => "tab",
        Scancode::LShift => "shift", Scancode::RShift => "shift",
        Scancode::LCtrl => "ctrl", Scancode::RCtrl => "ctrl",
        Scancode::LAlt => "alt", Scancode::RAlt => "alt",
        Scancode::Up => "up", Scancode::Down => "down",
        Scancode::Left => "left", Scancode::Right => "right",
        Scancode::Grave => "`",
        _ => return None,
    })
}
