use std::ops::Range;
use vt100::{Callbacks, Parser, Screen};

pub const SCROLLBACK_LINES: usize = 5_000;

/// A real cell grid, updated incrementally even when UTF-8 or escape sequences
/// arrive across multiple pipe reads.
pub struct Terminal {
    parser: Parser<Responses>,
    dirty: bool,
    selection: Option<Selection>,
    view_revision: u64,
}

struct Selection {
    // Ordered insertion boundaries in the visible grid. The end is exclusive.
    start: (u16, u16),
    end: (u16, u16),
    text: String,
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
            selection: None,
            view_revision: 0,
        }
    }

    /// Replies go back into ConPTY's input channel, never onto the display.
    pub fn process(&mut self, bytes: &[u8]) -> Vec<u8> {
        let alternate = self.screen().alternate_screen();
        self.parser.process(bytes);
        if alternate != self.screen().alternate_screen()
            || self.selection.as_ref().is_some_and(|s| {
                self.screen()
                    .contents_between(s.start.0, s.start.1, s.end.0, s.end.1)
                    != s.text
            })
        {
            self.change_view();
        }
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
        self.change_view();
        self.parser
            .screen_mut()
            .set_size(rows.max(1), columns.max(1));
        self.dirty = true;
    }

    pub fn scroll(&mut self, lines: i32) {
        let current = self.screen().scrollback();
        let next = current.saturating_add_signed(lines as isize);
        self.parser.screen_mut().set_scrollback(next);
        if self.screen().scrollback() != current {
            self.change_view();
        }
        self.dirty = true;
    }

    pub fn follow_output(&mut self) {
        self.parser.screen_mut().set_scrollback(0);
        self.change_view();
        self.dirty = true;
    }

    fn change_view(&mut self) {
        self.selection = None;
        self.view_revision = self.view_revision.wrapping_add(1);
    }

    pub fn view_revision(&self) -> u64 {
        self.view_revision
    }

    /// Select between two visible character boundaries, in either direction.
    pub fn select(&mut self, anchor: (u16, u16), end: (u16, u16)) {
        let (rows, cols) = self.screen().size();
        let clamp = |(r, c): (u16, u16)| (r.min(rows - 1), c.min(cols));
        let (anchor, end) = (clamp(anchor), clamp(end));
        let (start, end) = (anchor.min(end), anchor.max(end));
        self.selection = (start != end).then(|| Selection {
            start,
            end,
            text: self
                .screen()
                .contents_between(start.0, start.1, end.0, end.1),
        });
        self.dirty = true;
    }

    pub fn selected_columns(&self, row: u16) -> Option<Range<u16>> {
        let s = self.selection.as_ref()?;
        if !(s.start.0..=s.end.0).contains(&row) {
            return None;
        }
        let left = if row == s.start.0 { s.start.1 } else { 0 };
        let right = if row == s.end.0 {
            s.end.1
        } else {
            self.screen().size().1
        };
        Some(left..right)
    }

    pub fn copy_contents(&self) -> String {
        self.selection
            .as_ref()
            .map_or_else(|| self.screen().contents(), |s| s.text.clone())
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
