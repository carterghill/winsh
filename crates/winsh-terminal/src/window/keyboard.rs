use crate::input::{self, Key, Modifiers};
use std::io::Write;
use windows_sys::Win32::{
    System::Console::{
        CAPSLOCK_ON, ENHANCED_KEY, LEFT_ALT_PRESSED, LEFT_CTRL_PRESSED, NUMLOCK_ON,
        RIGHT_ALT_PRESSED, RIGHT_CTRL_PRESSED, SCROLLLOCK_ON, SHIFT_PRESSED,
    },
    UI::Input::KeyboardAndMouse::*,
};

pub(super) struct KeyboardState(pub(super) [u8; 256]);

impl KeyboardState {
    pub fn read() -> Self {
        let mut keys = [0; 256];
        // SAFETY: the buffer holds all 256 virtual-key states for this thread.
        unsafe { GetKeyboardState(keys.as_mut_ptr()) };
        Self(keys)
    }

    fn pressed(&self, code: u16) -> bool {
        self.0[usize::from(code)] & 0x80 != 0
    }

    pub fn modifiers(&self) -> Modifiers {
        Modifiers {
            shift: self.pressed(VK_SHIFT),
            alt: self.pressed(VK_MENU),
            control: self.pressed(VK_CONTROL),
        }
    }

    /// Encode one physical key message using ConPTY's win32-input-mode protocol.
    pub fn encode(&self, code: u16, lparam: isize, down: bool) -> Vec<u8> {
        let scan = (lparam as u32 >> 16) & 0xff;
        let repeat = (lparam as u16).max(1);
        let mut state = if lparam & (1 << 24) != 0 {
            ENHANCED_KEY
        } else {
            0
        };
        for (key, bit) in [
            (VK_LMENU, LEFT_ALT_PRESSED),
            (VK_RMENU, RIGHT_ALT_PRESSED),
            (VK_LCONTROL, LEFT_CTRL_PRESSED),
            (VK_RCONTROL, RIGHT_CTRL_PRESSED),
            (VK_SHIFT, SHIFT_PRESSED),
        ] {
            if self.pressed(key) {
                state |= bit;
            }
        }
        for (key, bit) in [
            (VK_CAPITAL, CAPSLOCK_ON),
            (VK_NUMLOCK, NUMLOCK_ON),
            (VK_SCROLL, SCROLLLOCK_ON),
        ] {
            if self.0[usize::from(key)] & 1 != 0 {
                state |= bit;
            }
        }

        // Translate exactly once so the active layout handles AltGr, dead keys,
        // and surrogate pairs. Only Alt release commits an Alt+numpad character.
        // Other releases query the layout without altering pending composition.
        let alt_release = !down && code == VK_MENU;
        let mut units = [0; 16];
        // SAFETY: both buffers have the lengths required by ToUnicode.
        let count = unsafe {
            ToUnicode(
                u32::from(code),
                if alt_release { scan | 0x8000 } else { scan },
                self.0.as_ptr(),
                units.as_mut_ptr(),
                units.len() as i32,
                if down || alt_release { 0 } else { 4 },
            )
        };
        // Non-text and dead keys still have a physical key event, with Uc=0.
        let characters = if count > 0 {
            &units[..count as usize]
        } else {
            &[0][..]
        };
        let mut bytes = Vec::new();
        for unit in characters {
            write_native_key(&mut bytes, code, scan, *unit, down, state, repeat);
        }
        bytes
    }
}

fn write_native_key(
    bytes: &mut Vec<u8>,
    code: u16,
    scan: u32,
    unit: u16,
    down: bool,
    state: u32,
    repeat: u16,
) {
    write!(
        bytes,
        "\x1b[{code};{scan};{unit};{};{state};{repeat}_",
        u8::from(down)
    )
    .expect("writing to Vec");
}

/// Match unmodified physical arrow presses, including releases for native readers.
pub(super) fn horizontal_steps(
    left: bool,
    count: usize,
    native: bool,
    application: bool,
) -> Vec<u8> {
    let key = if native {
        let (code, scan) = if left {
            (VK_LEFT, 0x4b)
        } else {
            (VK_RIGHT, 0x4d)
        };
        let mut key = Vec::new();
        write_native_key(&mut key, code, scan, 0, true, ENHANCED_KEY, 1);
        write_native_key(&mut key, code, scan, 0, false, ENHANCED_KEY, 1);
        key
    } else {
        input::key(
            if left { Key::Left } else { Key::Right },
            Modifiers::default(),
            application,
        )
    };
    key.repeat(count)
}

/// Older ConPTY versions take VT sequences. Returning None lets Windows
/// translate regular text, including dead keys and AltGr, via WM_CHAR.
pub(super) fn vt_key(code: u16, modifiers: Modifiers, application_cursor: bool) -> Option<Vec<u8>> {
    if code == VK_RETURN && modifiers.shift && !modifiers.control && !modifiers.alt {
        return Some(vec![b'\n']);
    }
    if (VK_A..=VK_Z).contains(&code) && modifiers.control && !modifiers.alt {
        return Some(vec![(code - VK_A + 1) as u8]);
    }
    let key = match code {
        VK_UP => Key::Up,
        VK_DOWN => Key::Down,
        VK_RIGHT => Key::Right,
        VK_LEFT => Key::Left,
        VK_HOME => Key::Home,
        VK_END => Key::End,
        VK_INSERT => Key::Insert,
        VK_DELETE => Key::Delete,
        VK_PRIOR => Key::PageUp,
        VK_NEXT => Key::PageDown,
        VK_TAB => Key::Tab,
        VK_F1..=VK_F12 => Key::Function((code - VK_F1 + 1) as u8),
        _ => return None,
    };
    Some(input::key(key, modifiers, application_cursor))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_input_encodes_controls_and_keeps_layout_text_on_the_character_path() {
        let control = Modifiers {
            control: true,
            ..Default::default()
        };
        for code in VK_A..=VK_Z {
            assert_eq!(
                vt_key(code, control, false),
                Some(vec![(code - VK_A + 1) as u8])
            );
        }
        let shift = Modifiers {
            shift: true,
            ..Default::default()
        };
        assert_eq!(vt_key(VK_RETURN, shift, false), Some(vec![b'\n']));
        assert_eq!(vt_key(VK_RETURN, Modifiers::default(), false), None);
        assert_eq!(
            vt_key(
                VK_Q,
                Modifiers {
                    alt: true,
                    ..control
                },
                false
            ),
            None
        );
    }
}
