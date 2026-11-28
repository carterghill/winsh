use super::{KeyboardState, Modifiers, PADDING, POINT, State, Terminal, keyboard, px};
use vt100::{MouseProtocolEncoding, MouseProtocolMode, Screen};
use windows_sys::Win32::UI::{
    HiDpi::GetSystemMetricsForDpi,
    WindowsAndMessaging::{SM_CXDRAG, SM_CYDRAG, WHEEL_DELTA},
};

type Position = (u16, u16);

#[derive(Clone, Copy)]
enum Button {
    Left = 0,
    WheelUp = 64,
    WheelDown = 65,
}

#[derive(Clone, Copy)]
struct Hit {
    cell: Position,
    boundary: Position,
    inside: bool,
}

pub(super) struct Gesture {
    origin: POINT,
    anchor: Hit,
    threshold: (i32, i32),
    dragged: bool,
    end: Position,
    view: u64,
    modifiers: Modifiers,
}

impl State {
    fn grid_hit(&self, screen: &Screen, pt: POINT) -> Hit {
        let padding = px(self.dpi, PADDING);
        hit_test(
            screen,
            pt.x - padding,
            pt.y - self.layout.height - padding,
            self.fonts.width,
            self.fonts.height,
        )
    }

    pub(super) fn mouse_wheel(&mut self, pt: POINT, delta: i16, modifiers: Modifiers) {
        let remainder = &mut self.current_mut().wheel;
        *remainder += i32::from(delta);
        let ticks = *remainder / WHEEL_DELTA as i32;
        *remainder %= WHEEL_DELTA as i32;
        if ticks == 0 {
            return;
        }

        let mut terminal = self.current().terminal.lock().unwrap();
        let hit = self.grid_hit(terminal.screen(), pt);
        let bytes = wheel_input(&mut terminal, hit, ticks, modifiers);
        drop(terminal);
        self.send(bytes);
    }

    pub(super) fn begin_gesture(&mut self, pt: POINT) -> bool {
        let mut terminal = self.current().terminal.lock().unwrap();
        let anchor = self.grid_hit(terminal.screen(), pt);
        if !anchor.inside {
            return false;
        }
        terminal.select(anchor.boundary, anchor.boundary);
        let view = terminal.view_revision();
        drop(terminal);
        // SAFETY: these system metrics require no window handle.
        let threshold = unsafe {
            (
                GetSystemMetricsForDpi(SM_CXDRAG, self.dpi).clamp(1, self.fonts.width),
                GetSystemMetricsForDpi(SM_CYDRAG, self.dpi).clamp(1, self.fonts.height),
            )
        };
        self.gesture = Some(Gesture {
            origin: pt,
            anchor,
            threshold,
            dragged: false,
            end: anchor.boundary,
            view,
            modifiers: KeyboardState::read().modifiers(),
        });
        true
    }

    pub(super) fn update_gesture(&mut self, pt: POINT, release: bool) {
        let Some(mut gesture) = self.gesture.take() else {
            return;
        };
        let mut terminal = self.current().terminal.lock().unwrap();
        if gesture.view != terminal.view_revision() {
            return;
        }
        let hit = self.grid_hit(terminal.screen(), pt);
        gesture.dragged |= (pt.x - gesture.origin.x).abs() >= gesture.threshold.0
            || (pt.y - gesture.origin.y).abs() >= gesture.threshold.1;
        if gesture.dragged && gesture.end != hit.boundary {
            terminal.select(gesture.anchor.boundary, hit.boundary);
            gesture.end = hit.boundary;
        }
        if release && !gesture.dragged && hit.inside {
            let bytes = click_input(&terminal, gesture.anchor, gesture.modifiers);
            drop(terminal);
            self.send(bytes);
        } else if !release {
            drop(terminal);
            self.gesture = Some(gesture);
        }
    }
}

fn hit_test(screen: &Screen, x: i32, y: i32, width: i32, height: i32) -> Hit {
    let (rows, cols) = screen.size();
    let right = i32::from(cols) * width;
    let inside = (0..right).contains(&x) && (0..i32::from(rows) * height).contains(&y);
    let row = (y / height).clamp(0, i32::from(rows - 1)) as u16;
    let col = (x / width).clamp(0, i32::from(cols - 1)) as u16;
    let cell = screen.cell(row, col).unwrap();
    // Snap to the nearest edge of the whole glyph, including double-width cells.
    let after =
        x >= right || cell.is_wide_continuation() || (!cell.is_wide() && (x % width) * 2 >= width);
    Hit {
        cell: (row, col),
        boundary: (row, col + u16::from(after)),
        inside,
    }
}

fn wheel_input(terminal: &mut Terminal, hit: Hit, ticks: i32, modifiers: Modifiers) -> Vec<u8> {
    let screen = terminal.screen();
    if screen.scrollback() != 0 || screen.mouse_protocol_mode() == MouseProtocolMode::None {
        terminal.scroll(ticks * 3);
        return Vec::new();
    }
    if !hit.inside {
        return Vec::new();
    }
    let button = if ticks > 0 {
        Button::WheelUp
    } else {
        Button::WheelDown
    };
    mouse_report(screen, hit.cell, modifiers, button).repeat(ticks.unsigned_abs() as usize)
}

fn click_input(terminal: &Terminal, hit: Hit, modifiers: Modifiers) -> Vec<u8> {
    let screen = terminal.screen();
    if screen.scrollback() != 0 {
        return Vec::new();
    }
    if screen.mouse_protocol_mode() != MouseProtocolMode::None {
        return mouse_report(screen, hit.cell, modifiers, Button::Left);
    }
    if screen.hide_cursor() {
        return Vec::new();
    }

    let cursor = screen.cursor_position();
    let (row, mut col) = hit.boundary;
    let (_, cols) = screen.size();
    // Only cross rows joined by terminal wrapping. Up/Down could recall a
    // command from history or activate a completely different UI control.
    if !(row.min(cursor.0)..row.max(cursor.0)).all(|r| screen.row_wrapped(r)) {
        return Vec::new();
    }
    if !screen.row_wrapped(row) {
        let text_end = (0..cols)
            .rfind(|&c| {
                screen
                    .cell(row, c)
                    .is_some_and(|cell| cell.has_contents() || cell.is_wide_continuation())
            })
            .map_or(0, |c| c + 1);
        col = col.min(text_end.max(if row == cursor.0 { cursor.1 } else { 0 }));
    }
    let target = (row, col);
    let start = cursor.min(target);
    let end = cursor.max(target);
    let mut steps = 0;
    for r in start.0..=end.0 {
        let from = if r == start.0 { start.1 } else { 0 };
        let to = if r == end.0 { end.1 } else { cols };
        steps += (from..to)
            .filter(|&c| {
                screen
                    .cell(r, c)
                    .is_some_and(|cell| !cell.is_wide_continuation())
            })
            .count();
    }
    keyboard::horizontal_steps(
        target < cursor,
        steps,
        terminal.win32_input_mode(),
        screen.application_cursor(),
    )
}

fn mouse_report(
    screen: &Screen,
    (row, col): Position,
    modifiers: Modifiers,
    button: Button,
) -> Vec<u8> {
    let press_only = screen.mouse_protocol_mode() == MouseProtocolMode::Press;
    let release = matches!(button, Button::Left) && !press_only;
    let modifier_bits = if press_only {
        0
    } else {
        4 * u8::from(modifiers.shift)
            + 8 * u8::from(modifiers.alt)
            + 16 * u8::from(modifiers.control)
    };
    let code = button as u8 + modifier_bits;
    let x = u32::from(col) + 1;
    let y = u32::from(row) + 1;
    let encoding = screen.mouse_protocol_encoding();
    if encoding == MouseProtocolEncoding::Sgr {
        let mut bytes = format!("\x1b[<{code};{x};{y}M").into_bytes();
        if release {
            bytes.extend(format!("\x1b[<{code};{x};{y}m").bytes());
        }
        return bytes;
    }
    let limit = if encoding == MouseProtocolEncoding::Utf8 {
        2015
    } else {
        223
    };
    if x > limit || y > limit {
        return Vec::new();
    }
    let mut bytes = Vec::new();
    for code in [code, modifier_bits + 3]
        .into_iter()
        .take(if release { 2 } else { 1 })
    {
        bytes.extend_from_slice(b"\x1b[M");
        bytes.push(32 + code);
        for coord in [x, y] {
            if encoding == MouseProtocolEncoding::Utf8 {
                let ch = char::from_u32(32 + coord).unwrap();
                bytes.extend_from_slice(ch.encode_utf8(&mut [0; 4]).as_bytes());
            } else {
                bytes.push((32 + coord) as u8);
            }
        }
    }
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        path::PathBuf,
        thread,
        time::{Duration, Instant},
    };

    #[test]
    fn keyboard_clicks_count_glyphs_on_the_cursor_line() {
        let mut t = Terminal::new(3, 5);
        t.process("a界ce\u{301}fg".as_bytes());
        let click = |t: &Terminal, x, y| {
            click_input(t, hit_test(t.screen(), x, y, 10, 20), Modifiers::default())
        };
        assert_eq!(click(&t, 10, 0), b"\x1b[D".repeat(5));
        assert_eq!(click(&t, 20, 0), b"\x1b[D".repeat(4));
        assert!(click(&t, 0, 40).is_empty());
        t.process(b"\x1b[?1h\x1b[2;1H");
        assert_eq!(click(&t, 49, 20), b"\x1bOC".repeat(2));
        t.process(b"\x1b[?25l");
        assert!(click(&t, 10, 20).is_empty());
    }

    #[test]
    fn mouse_aware_apps_receive_the_requested_click_encoding() {
        let mut t = Terminal::new(3, 300);
        let click = |t: &Terminal, col| {
            click_input(
                t,
                hit_test(t.screen(), col * 10, 20, 10, 20),
                Modifiers::default(),
            )
        };
        t.process(b"\x1b[?9h");
        assert_eq!(click(&t, 4), b"\x1b[M %\"");
        t.process(b"\x1b[?1000h");
        assert_eq!(click(&t, 4), b"\x1b[M %\"\x1b[M#%\"");
        assert!(click(&t, 223).is_empty());
        t.process(b"\x1b[?1005h");
        assert_eq!(click(&t, 223), "\x1b[M Ā\"\x1b[M#Ā\"".as_bytes());
        t.process(b"\x1b[?1003;1006h\x1b[?25l");
        assert_eq!(click(&t, 223), b"\x1b[<0;224;2M\x1b[<0;224;2m");
        t.process(b"\r\n\r\n\r\n");
        t.scroll(1);
        assert!(click(&t, 4).is_empty());
    }

    #[test]
    fn wheel_switches_between_scrollback_and_application_input() {
        let mut t = Terminal::new(3, 20);
        t.process(b"0\r\n1\r\n2\r\n3\r\n4\r\n5");
        let hit = hit_test(t.screen(), 40, 20, 10, 20);
        let mods = Modifiers::default();
        assert!(wheel_input(&mut t, hit, 1, mods).is_empty());
        assert_eq!(t.screen().scrollback(), 3);
        t.process(b"\x1b[?1000h");
        assert!(wheel_input(&mut t, hit, -1, mods).is_empty());
        assert_eq!(t.screen().scrollback(), 0);
        assert_eq!(wheel_input(&mut t, hit, -1, mods), b"\x1b[Ma%\"");

        t.process(b"\x1b[?1049h\x1b[?1003;1006h");
        assert_eq!(
            wheel_input(&mut t, hit, 2, mods),
            b"\x1b[<64;5;2M".repeat(2)
        );
        let ctrl = Modifiers {
            control: true,
            ..mods
        };
        assert_eq!(wheel_input(&mut t, hit, -1, ctrl), b"\x1b[<81;5;2M");
        let padding = hit_test(t.screen(), -1, 20, 10, 20);
        assert!(wheel_input(&mut t, padding, 1, mods).is_empty());
        t.process(b"\x1b[?1003;1006l\x1b[?1049l");
        assert!(wheel_input(&mut t, hit, 1, mods).is_empty());
        assert_eq!(t.screen().scrollback(), 3);
    }

    fn find_text(state: &State, needle: &str) -> (u16, u16) {
        let deadline = Instant::now() + Duration::from_secs(8);
        loop {
            let terminal = state.current().terminal.lock().unwrap();
            let screen = terminal.screen();
            for (row, text) in screen.rows(0, screen.size().1).enumerate() {
                if text.contains(needle) {
                    let row = row as u16;
                    if let Some(col) = (0..screen.size().1).find(|&c| {
                        screen.contents_between(row, c, row, c + needle.len() as u16) == needle
                    }) {
                        return (row, col);
                    }
                }
            }
            if Instant::now() >= deadline {
                let text = screen.contents();
                drop(terminal);
                panic!("waiting for {needle:?}: {text}");
            }
            drop(terminal);
            thread::sleep(Duration::from_millis(15));
        }
    }

    // Exercise the same press, move and release handlers used by the window
    // with a real line editor at the other end of the input pipe.
    fn exercise_editor(state: &mut State) {
        state.send(b"abcdef".to_vec());
        let point = |state: &State, (row, col): Position, offset| POINT {
            x: px(state.dpi, PADDING) + (i32::from(col) + offset) * state.fonts.width + 1,
            y: state.layout.height
                + px(state.dpi, PADDING)
                + i32::from(row) * state.fonts.height
                + 1,
        };
        let pos = find_text(state, "abcdef");
        let (a, b) = (point(state, pos, 2), point(state, pos, 5));
        assert!(state.begin_gesture(a));
        state.update_gesture(b, false);
        assert_eq!(
            state.current().terminal.lock().unwrap().copy_contents(),
            "cde"
        );
        state.update_gesture(b, true);
        assert_eq!(
            state.current().terminal.lock().unwrap().copy_contents(),
            "cde"
        );
        // Drag away and back, then release at the original click location.
        assert!(state.begin_gesture(a));
        state.update_gesture(b, false);
        state.update_gesture(a, true);
        state.send(b"X".to_vec());
        let pos = find_text(state, "abcdefX");
        let a = point(state, pos, 2);
        assert!(state.begin_gesture(a));
        state.update_gesture(a, true);
        state.send(b"Y".to_vec());
        find_text(state, "abYcdefX");
    }

    #[test]
    fn dragging_keeps_the_editing_cursor_and_clicking_repositions_it() {
        let program = PathBuf::from(std::env::var_os("ComSpec").unwrap());
        let mut state = State::new(&program, &["/d".into(), "/q".into()], 96, None).unwrap();
        state.send(b"prompt edit$G\r".to_vec());
        find_text(&state, "edit>");
        exercise_editor(&mut state);
    }

    #[test]
    #[ignore = "requires a configured interactive editor and its ready text"]
    fn external_editor_click_and_drag() {
        let program = PathBuf::from(std::env::var_os("WINSH_TEST_EDITOR").unwrap());
        let mut state = State::new(&program, &[], 96, None).unwrap();
        find_text(&state, &std::env::var("WINSH_TEST_EDITOR_READY").unwrap());
        exercise_editor(&mut state);
    }
}
