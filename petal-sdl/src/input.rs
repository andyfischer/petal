use sdl2::keyboard::Scancode;
use std::collections::HashSet;

#[derive(Default)]
pub struct InputState {
    pub keys_down: HashSet<String>,
    pub keys_prev: HashSet<String>,
    pub mouse_x: i32,
    pub mouse_y: i32,
    pub mouse_buttons: HashSet<u8>,
}

impl InputState {
    pub fn key_down(&self, name: &str) -> bool {
        self.keys_down.contains(name)
    }

    pub fn key_pressed(&self, name: &str) -> bool {
        self.keys_down.contains(name) && !self.keys_prev.contains(name)
    }

    pub fn mouse_down(&self, button: u8) -> bool {
        self.mouse_buttons.contains(&button)
    }

    pub fn begin_frame(&mut self) {
        self.keys_prev = self.keys_down.clone();
    }
}

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
        Scancode::LShift | Scancode::RShift => "shift",
        Scancode::LCtrl | Scancode::RCtrl => "ctrl",
        Scancode::LAlt | Scancode::RAlt => "alt",
        _ => return None,
    })
}
