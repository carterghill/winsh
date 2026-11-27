use winsh_terminal::{
    input::{self, Key, Modifiers, TextInput},
    terminal::Terminal,
};

#[test]
fn terminal_interprets_cursor_movement_erase_and_attributes() {
    let mut terminal = Terminal::new(5, 20);
    terminal.process(b"first\r\nsecond\x1b[1;1H\x1b[2K\x1b[31;1mred");
    assert_eq!(terminal.screen().contents(), "red\nsecond");
    let cell = terminal.screen().cell(0, 0).unwrap();
    assert_eq!(cell.fgcolor(), vt100::Color::Idx(1));
    assert!(cell.bold());
    terminal.process(b"\x1b[38;2;12;34;56mX");
    assert_eq!(
        terminal.screen().cell(0, 3).unwrap().fgcolor(),
        vt100::Color::Rgb(12, 34, 56)
    );
}

#[test]
fn split_utf8_and_escape_sequences_preserve_unicode_cells() {
    let mut terminal = Terminal::new(5, 20);
    for byte in "\x1b[2;3H日本語 e\u{301} 🦀".as_bytes() {
        terminal.process(&[*byte]);
    }
    assert_eq!(terminal.screen().cell(1, 2).unwrap().contents(), "日");
    assert!(terminal.screen().cell(1, 2).unwrap().is_wide());
    assert!(terminal.screen().cell(1, 3).unwrap().is_wide_continuation());
    assert!(terminal.screen().contents().contains("日本語 e\u{301} 🦀"));
}

#[test]
fn alternate_screen_scrollback_and_resize_keep_screen_state() {
    let mut terminal = Terminal::new(3, 20);
    terminal.process(b"one\r\ntwo\r\nthree\r\nfour");
    terminal.scroll(1);
    assert!(terminal.screen().contents().starts_with("one"));
    terminal.follow_output();
    assert!(terminal.screen().contents().starts_with("two"));
    terminal.process(b"\x1b[?1049happlication");
    assert!(terminal.screen().alternate_screen());
    assert!(!terminal.screen().contents().contains("two"));
    terminal.process(b"\x1b[?1049l");
    assert!(!terminal.screen().alternate_screen());
    assert!(terminal.screen().contents().contains("four"));
    terminal.resize(5, 30);
    assert_eq!(terminal.screen().size(), (5, 30));
    terminal.resize(0, 0);
    assert_eq!(terminal.screen().size(), (1, 1));
}

#[test]
fn selection_copies_visible_unicode_across_soft_wraps_and_hard_breaks() {
    let mut t = Terminal::new(3, 5);
    t.process("a界cde\u{301}f\r\nend".as_bytes());
    t.select((2, 2), (0, 1));
    assert_eq!(t.copy_contents(), "界cde\u{301}f\nen");
    assert_eq!(t.selected_columns(0), Some(1..5));
    assert_eq!(t.selected_columns(1), Some(0..5));
    assert_eq!(t.selected_columns(2), Some(0..2));
    t.process(b"!");
    assert_eq!(t.copy_contents(), "界cde\u{301}f\nen");
    t.process(b"\r\nnext");
    assert_eq!(t.selected_columns(0), None);
    t.scroll(1);
    t.select((0, 1), (0, 3));
    assert_eq!(t.copy_contents(), "界");
    t.follow_output();
    assert_eq!(t.copy_contents(), t.screen().contents());
    t.select((0, 0), (1, 2));
    t.process(b"\x1b[?1049h");
    assert_eq!(t.selected_columns(0), None);
}

#[test]
fn terminal_answers_queries_and_sanitizes_window_titles() {
    let mut terminal = Terminal::new(24, 80);
    let response = terminal.process(b"\x1b[3;7H\x1b[6n\x1b[5n\x1b[c\x1b[18t");
    assert_eq!(response, b"\x1b[3;7R\x1b[0n\x1b[?1;2c\x1b[8;24;80t");
    terminal.process(b"\x1b]2;project\x07");
    assert_eq!(terminal.take_title().as_deref(), Some("project"));
    assert_eq!(terminal.take_title(), None);
    terminal.process(b"\x1b]0;work;archive\x07");
    assert_eq!(terminal.take_title().as_deref(), Some("work;archive"));
}

#[test]
fn conpty_selects_native_keyboard_input_per_terminal() {
    let mut terminal = Terminal::new(5, 20);
    terminal.process(b"\x1b[?1;90");
    assert!(!terminal.win32_input_mode());
    terminal.process(b"01h");
    assert!(terminal.win32_input_mode());
    assert!(terminal.screen().application_cursor());
    assert!(!Terminal::new(5, 20).win32_input_mode());
    terminal.process(b"\x1b[?1l");
    assert!(terminal.win32_input_mode());
    terminal.process(b"\x1b[?9001l");
    assert!(!terminal.win32_input_mode());
}

#[test]
fn keyboard_modes_modifiers_unicode_and_paste_are_encoded_for_the_child() {
    assert_eq!(input::key(Key::Up, Modifiers::default(), false), b"\x1b[A");
    assert_eq!(input::key(Key::Up, Modifiers::default(), true), b"\x1bOA");
    assert_eq!(
        input::key(
            Key::Left,
            Modifiers {
                control: true,
                ..Modifiers::default()
            },
            false
        ),
        b"\x1b[1;5D"
    );
    assert_eq!(
        input::key(Key::Function(12), Modifiers::default(), false),
        b"\x1b[24~"
    );
    assert_eq!(input::paste("one\r\ntwo\n", false), b"one\rtwo\r");
    assert_eq!(input::paste("hello", true), b"\x1b[200~hello\x1b[201~");
    let mut text = TextInput::default();
    assert!(text.push(0xd83e).is_empty());
    assert_eq!(text.push(0xdd80), "🦀");
    assert!(text.push(0xd800).is_empty());
    assert_eq!(text.push(b'a' as u16), "\u{fffd}a");
}
