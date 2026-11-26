//! Encode terminal keys independently of the Windows event loop.

#[derive(Clone, Copy, Debug)]
pub enum Key {
    Up,
    Down,
    Right,
    Left,
    Home,
    End,
    Insert,
    Delete,
    PageUp,
    PageDown,
    Function(u8),
    Tab,
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct Modifiers {
    pub shift: bool,
    pub alt: bool,
    pub control: bool,
}

pub fn key(key: Key, modifiers: Modifiers, application_cursor: bool) -> Vec<u8> {
    let modifier = 1
        + u8::from(modifiers.shift)
        + 2 * u8::from(modifiers.alt)
        + 4 * u8::from(modifiers.control);
    let (final_char, number) = match key {
        Key::Up => ('A', None),
        Key::Down => ('B', None),
        Key::Right => ('C', None),
        Key::Left => ('D', None),
        Key::Home => ('H', None),
        Key::End => ('F', None),
        Key::Insert => ('~', Some(2)),
        Key::Delete => ('~', Some(3)),
        Key::PageUp => ('~', Some(5)),
        Key::PageDown => ('~', Some(6)),
        Key::Function(n @ 1..=4) => ((b'P' + n - 1) as char, None),
        Key::Function(n @ 5..=12) => (
            '~',
            Some([15, 17, 18, 19, 20, 21, 23, 24][usize::from(n - 5)]),
        ),
        Key::Function(_) => return Vec::new(),
        Key::Tab => {
            return if modifiers.shift {
                b"\x1b[Z".to_vec()
            } else {
                vec![b'\t']
            };
        }
    };
    let sequence = if let Some(number) = number {
        if modifier == 1 {
            format!("\x1b[{number}~")
        } else {
            format!("\x1b[{number};{modifier}~")
        }
    } else if modifier != 1 {
        format!("\x1b[1;{modifier}{final_char}")
    } else if application_cursor || matches!(key, Key::Function(_)) {
        format!("\x1bO{final_char}")
    } else {
        format!("\x1b[{final_char}")
    };
    sequence.into_bytes()
}

/// Decode WM_CHAR's UTF-16, including non-BMP input delivered in two messages.
#[derive(Default)]
pub struct TextInput {
    high_surrogate: Option<u16>,
}

impl TextInput {
    pub fn push(&mut self, unit: u16) -> String {
        let mut text = String::new();
        if let Some(high) = self.high_surrogate.take() {
            if (0xdc00..=0xdfff).contains(&unit) {
                let scalar =
                    0x10000 + ((u32::from(high) - 0xd800) << 10) + u32::from(unit) - 0xdc00;
                text.push(char::from_u32(scalar).unwrap_or(char::REPLACEMENT_CHARACTER));
                return text;
            }
            text.push(char::REPLACEMENT_CHARACTER);
        }
        if (0xd800..=0xdbff).contains(&unit) {
            self.high_surrogate = Some(unit);
        } else {
            text.push(char::from_u32(u32::from(unit)).unwrap_or(char::REPLACEMENT_CHARACTER));
        }
        text
    }
}

pub fn paste(text: &str, bracketed: bool) -> Vec<u8> {
    let text = text.replace("\r\n", "\n").replace('\n', "\r");
    if bracketed {
        format!("\x1b[200~{text}\x1b[201~").into_bytes()
    } else {
        text.into_bytes()
    }
}
