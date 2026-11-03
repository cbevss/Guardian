//! Mouse and keyboard injection. Real input on Windows (enigo); elsewhere it
//! just logs, so the protocol can be tested on any machine.
//! Keys arrive as browser KeyboardEvent.key values ("a", "Enter", "ArrowLeft"...).

pub struct Input {
    held: Vec<String>, // keys currently held down, released on disconnect
    #[cfg(windows)]
    enigo: Option<enigo::Enigo>,
}

impl Input {
    pub fn new() -> Self {
        Self {
            held: Vec::new(),
            #[cfg(windows)]
            enigo: enigo::Enigo::new(&enigo::Settings::default()).ok(),
        }
    }

    /// Never leave Shift/Ctrl stuck on his PC if the connection drops.
    pub fn release_all(&mut self) {
        for k in std::mem::take(&mut self.held) { self.key_raw(&k, false); }
    }

    pub fn key(&mut self, key: &str, down: bool) {
        if key.is_empty() || key.chars().count() > 12 { return; }
        if down { if !self.held.iter().any(|k| k == key) { self.held.push(key.to_owned()); } }
        else { self.held.retain(|k| k != key); }
        self.key_raw(key, down);
    }
}

#[cfg(windows)]
mod imp {
    use super::Input;
    use enigo::{Axis, Button, Coordinate, Direction, Key, Keyboard, Mouse};

    fn map_key(k: &str) -> Option<Key> {
        let mut chars = k.chars();
        if let (Some(c), None) = (chars.next(), chars.next()) { return Some(Key::Unicode(c)); }
        Some(match k {
            "Enter" => Key::Return, "Backspace" => Key::Backspace, "Tab" => Key::Tab,
            "Escape" => Key::Escape, "Delete" => Key::Delete, "Home" => Key::Home, "End" => Key::End,
            "PageUp" => Key::PageUp, "PageDown" => Key::PageDown,
            "ArrowLeft" => Key::LeftArrow, "ArrowRight" => Key::RightArrow,
            "ArrowUp" => Key::UpArrow, "ArrowDown" => Key::DownArrow,
            "Shift" => Key::Shift, "Control" => Key::Control, "Alt" => Key::Alt, "Meta" => Key::Meta,
            "CapsLock" => Key::CapsLock, " " => Key::Space,
            "F1" => Key::F1, "F2" => Key::F2, "F3" => Key::F3, "F4" => Key::F4, "F5" => Key::F5, "F6" => Key::F6,
            "F7" => Key::F7, "F8" => Key::F8, "F9" => Key::F9, "F10" => Key::F10, "F11" => Key::F11, "F12" => Key::F12,
            _ => return None,
        })
    }

    impl Input {
        pub fn mouse_move(&mut self, x: i32, y: i32) {
            if let Some(e) = &mut self.enigo { let _ = e.move_mouse(x, y, Coordinate::Abs); }
        }
        pub fn mouse_button(&mut self, b: &str, down: bool) {
            let btn = match b { "left" => Button::Left, "right" => Button::Right, "middle" => Button::Middle, _ => return };
            let dir = if down { Direction::Press } else { Direction::Release };
            if let Some(e) = &mut self.enigo { let _ = e.button(btn, dir); }
        }
        pub fn wheel(&mut self, dx: i32, dy: i32) {
            if let Some(e) = &mut self.enigo {
                if dy != 0 { let _ = e.scroll(dy, Axis::Vertical); }
                if dx != 0 { let _ = e.scroll(dx, Axis::Horizontal); }
            }
        }
        pub(super) fn key_raw(&mut self, k: &str, down: bool) {
            let (Some(key), Some(e)) = (map_key(k), &mut self.enigo) else { return };
            let _ = e.key(key, if down { Direction::Press } else { Direction::Release });
        }
    }
}

#[cfg(not(windows))]
impl Input {
    pub fn mouse_move(&mut self, x: i32, y: i32) { println!("[input] move {x},{y}"); }
    pub fn mouse_button(&mut self, b: &str, down: bool) { println!("[input] {b} {}", if down { "down" } else { "up" }); }
    pub fn wheel(&mut self, dx: i32, dy: i32) { println!("[input] wheel {dx},{dy}"); }
    fn key_raw(&mut self, k: &str, down: bool) { println!("[input] key {k:?} {}", if down { "down" } else { "up" }); }
}
