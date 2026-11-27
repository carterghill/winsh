use vt100::{Callbacks, Parser, Screen};

pub const SCROLLBACK_LINES: usize = 5_000;

/// A real cell grid, updated incrementally even when UTF-8 or escape sequences
/// arrive across multiple pipe reads.
pub struct Terminal {
    parser: Parser<Responses>,
    dirty: bool,
}

impl Terminal {
    pub fn new(rows: u16, columns: u16) -> Self {
        Self {
            parser: Parser::new_with_callbacks(
                rows.max(1),
                columns.max(1),
                SCROLLBACK_LINES,
                Responses::default(),
            ),
            dirty: true,
        }
    }

    /// Replies go back into ConPTY's input channel, never onto the display.
    pub fn process(&mut self, bytes: &[u8]) -> Vec<u8> {
        self.parser.process(bytes);
        self.dirty = true;
        std::mem::take(&mut self.parser.callbacks_mut().bytes)
    }

    pub fn screen(&self) -> &Screen {
        self.parser.screen()
    }

    pub fn win32_input_mode(&self) -> bool {
        self.parser.callbacks().win32_input_mode
    }

    pub fn resize(&mut self, rows: u16, columns: u16) {
        self.parser
            .screen_mut()
            .set_size(rows.max(1), columns.max(1));
        self.dirty = true;
    }

    pub fn scroll(&mut self, lines: i32) {
        let current = self.screen().scrollback();
        let next = current.saturating_add_signed(lines as isize);
        self.parser.screen_mut().set_scrollback(next);
        self.dirty = true;
    }

    pub fn follow_output(&mut self) {
        self.parser.screen_mut().set_scrollback(0);
        self.dirty = true;
    }

    pub fn take_dirty(&mut self) -> bool {
        std::mem::take(&mut self.dirty)
    }

    pub fn take_title(&mut self) -> Option<String> {
        self.parser.callbacks_mut().title.take()
    }
}

#[derive(Default)]
struct Responses {
    bytes: Vec<u8>,
    title: Option<String>,
    win32_input_mode: bool,
}

impl Callbacks for Responses {
    fn set_window_title(&mut self, _: &mut Screen, title: &[u8]) {
        self.title = Some(
            String::from_utf8_lossy(title)
                .chars()
                .filter(|ch| !ch.is_control())
                .take(256)
                .collect(),
        );
    }

    fn unhandled_osc(&mut self, screen: &mut Screen, params: &[&[u8]]) {
        // vt100 splits on every semicolon, including ones inside a folder name.
        if let [b"0" | b"2", parts @ ..] = params
            && parts.len() > 1
        {
            self.set_window_title(screen, &parts.join(&b';'));
        }
    }

    fn unhandled_escape(&mut self, _: &mut Screen, i1: Option<u8>, _: Option<u8>, ch: u8) {
        if i1.is_none() && ch == b'Z' {
            self.bytes.extend_from_slice(b"\x1b[?1;2c");
        }
    }

    fn unhandled_csi(
        &mut self,
        screen: &mut Screen,
        i1: Option<u8>,
        i2: Option<u8>,
        params: &[&[u16]],
        ch: char,
    ) {
        if i2.is_some() {
            return;
        }
        if i1 == Some(b'?') && matches!(ch, 'h' | 'l') && params.contains(&&[9001][..]) {
            self.win32_input_mode = ch == 'h';
            return;
        }
        let parameter = params.first().and_then(|p| p.first()).copied().unwrap_or(0);
        let response = match (i1, parameter, ch) {
            (None, 5, 'n') => "\x1b[0n".to_owned(),
            (None | Some(b'?'), 6, 'n') => {
                let (row, column) = screen.cursor_position();
                let private = if i1.is_some() { "?" } else { "" };
                format!("\x1b[{private}{};{}R", row + 1, column + 1)
            }
            (None, 0, 'c') => "\x1b[?1;2c".to_owned(),
            (Some(b'>'), 0, 'c') => "\x1b[>0;1;0c".to_owned(),
            (None, 18, 't') => {
                let (rows, columns) = screen.size();
                format!("\x1b[8;{rows};{columns}t")
            }
            _ => return,
        };
        self.bytes.extend_from_slice(response.as_bytes());
    }
}
