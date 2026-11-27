//! The native window and GDI renderer. All pointers and GDI objects stay on the
//! window thread; the session only shares the terminal grid through a mutex.
use std::{
    ffi::{OsStr, OsString},
    io,
    os::windows::ffi::OsStrExt,
    path::{Path, PathBuf},
    ptr,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use windows_sys::{
    Win32::{
        Foundation::{GlobalFree, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM},
        Graphics::Gdi::*,
        System::{
            DataExchange::{
                CloseClipboard, EmptyClipboard, GetClipboardData, OpenClipboard, SetClipboardData,
            },
            LibraryLoader::GetModuleHandleW,
            Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock},
        },
        UI::{
            Controls::WM_MOUSELEAVE,
            HiDpi::{
                AdjustWindowRectExForDpi, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
                GetDpiForSystem, GetSystemMetricsForDpi, SetProcessDpiAwarenessContext,
            },
            Input::KeyboardAndMouse::*,
            WindowsAndMessaging::*,
        },
    },
    core::w,
};

use crate::{
    input::{self, Key, Modifiers, TextInput},
    session::{Event, Session},
    terminal::Terminal,
};

mod fonts;
mod keyboard;
mod keymap;
mod mouse;
mod settings;
mod titlebar;
use fonts::{Fonts, Symbols};
use keyboard::KeyboardState;
use keymap::{Action, Keymap};
use settings::{ConfigFile, FontSettings};
use titlebar::{Control, Layout, px};

const PADDING: i32 = 12;
const CF_UNICODETEXT: u32 = 13;
const DEFAULT_FOREGROUND: u32 = rgb(220, 224, 232);
const DEFAULT_BACKGROUND: u32 = rgb(20, 23, 29);

pub fn run(program: &Path, arguments: &[OsString]) -> io::Result<i32> {
    // SAFETY: called before creating windows. An existing DPI context is fine.
    unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    // SAFETY: null obtains this executable's loaded module; no ownership transfer.
    let instance = unsafe { GetModuleHandleW(ptr::null()) };
    // SAFETY: GetDpiForSystem has no preconditions.
    let dpi = unsafe { GetDpiForSystem() }.max(96);
    let config_dir = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .map(|p| p.join("winsh"));
    let mut state = Box::new(State::new(program, arguments, dpi, config_dir)?);
    // SAFETY: static system cursor, borrowed for the window class lifetime.
    let cursor = unsafe { LoadCursorW(ptr::null_mut(), IDC_IBEAM) };
    // SAFETY: 101 is the integer icon resource in windows/winsh.rc.
    // LoadIconW returns a shared handle that lives with the module.
    let icon = unsafe { LoadIconW(instance, 101 as *const u16) };
    let class = WNDCLASSEXW {
        cbSize: size_of::<WNDCLASSEXW>() as u32,
        style: CS_HREDRAW | CS_VREDRAW | CS_DBLCLKS,
        lpfnWndProc: Some(window_proc),
        hInstance: instance,
        hIcon: icon,
        hCursor: cursor,
        lpszClassName: w!("WinshTerminalWindow"),
        ..Default::default()
    };
    // SAFETY: class fields have valid static pointers, module, and callback.
    if unsafe { RegisterClassExW(&class) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let mut rectangle = RECT {
        left: 0,
        top: 0,
        right: 100 * state.fonts.width + 2 * px(dpi, PADDING),
        bottom: 30 * state.fonts.height + 2 * px(dpi, PADDING) + state.layout.height,
    };
    // SAFETY: initialized rectangle and the style used for CreateWindowExW.
    unsafe { AdjustWindowRectExForDpi(&mut rectangle, WS_OVERLAPPEDWINDOW, 0, 0, dpi) };
    // SAFETY: state is boxed and stays alive through the entire message loop.
    // WM_NCCREATE borrows its pointer; WM_NCDESTROY clears the window's copy.
    let hwnd = unsafe {
        CreateWindowExW(
            0,
            class.lpszClassName,
            w!("winsh"),
            WS_OVERLAPPEDWINDOW,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            rectangle.right - rectangle.left,
            rectangle.bottom, // The custom title bar occupies the client area.
            ptr::null_mut(),
            ptr::null_mut(),
            instance,
            (&mut *state as *mut State).cast(),
        )
    };
    if hwnd.is_null() {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: a live window; timer messages run only on this same thread.
    unsafe {
        if SetTimer(hwnd, 1, 16, None) == 0 {
            let error = io::Error::last_os_error();
            DestroyWindow(hwnd);
            return Err(error);
        }
        ShowWindow(hwnd, SW_SHOW);
        UpdateWindow(hwnd);
    }
    let mut message = MSG::default();
    let mut shortcut_keys = [false; 256];
    loop {
        // SAFETY: valid output MSG, no filtering, message dispatch on owner thread.
        let result = unsafe { GetMessageW(&mut message, ptr::null_mut(), 0, 0) };
        if result == 0 {
            break;
        }
        if result == -1 {
            let error = io::Error::last_os_error();
            // SAFETY: clear the live window's borrowed State pointer before
            // unwinding the Box or showing any subsequent error dialog.
            unsafe { DestroyWindow(hwnd) };
            return Err(error);
        }
        // SAFETY: a message obtained from GetMessageW.
        unsafe {
            if message.hwnd == hwnd
                && matches!(
                    message.message,
                    WM_KEYDOWN | WM_SYSKEYDOWN | WM_KEYUP | WM_SYSKEYUP
                )
                && message.wParam < shortcut_keys.len()
            {
                let code = message.wParam as u16;
                let down = matches!(message.message, WM_KEYDOWN | WM_SYSKEYDOWN);
                let keyboard = KeyboardState::read();
                let shortcut = &mut shortcut_keys[message.wParam];
                if down {
                    *shortcut = state.shortcut(
                        code,
                        keyboard.modifiers(),
                        message.lParam & (1 << 30) != 0,
                        hwnd,
                    );
                    if *shortcut {
                        refresh(hwnd, &mut *state, true);
                        continue;
                    }
                } else if std::mem::take(shortcut) {
                    continue;
                }
                // Let Windows deliver IME/injected text and handle system
                // menu/close commands through the usual message translation.
                let system_key = matches!(message.message, WM_SYSKEYDOWN | WM_SYSKEYUP)
                    && matches!(code, VK_F4 | VK_SPACE);
                if !system_key
                    && !matches!(code, VK_PROCESSKEY | VK_PACKET)
                    && state.terminal_key(code, message.lParam, down, &keyboard)
                {
                    // Translation would also post a WM_CHAR for this key.
                    continue;
                }
            }
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
    if let Some(error) = state.error.take() {
        return Err(io::Error::other(error));
    }
    Ok(state.status)
}

pub fn message(text: &str, error: bool) {
    window_message(ptr::null_mut(), text, error);
}

fn window_message(hwnd: HWND, text: &str, error: bool) {
    let text = wide(text);
    // SAFETY: both strings are terminated and live through this synchronous call.
    unsafe {
        MessageBoxW(
            hwnd,
            text.as_ptr(),
            w!("winsh terminal"),
            MB_OK
                | if error {
                    MB_ICONERROR
                } else {
                    MB_ICONINFORMATION
                },
        )
    };
}

struct Tab {
    terminal: Arc<Mutex<Terminal>>,
    session: Session,
    title: String,
    text: TextInput,
    wheel: i32,
}

impl Tab {
    fn spawn(
        program: &Path,
        arguments: &[OsString],
        (rows, columns): (u16, u16),
    ) -> io::Result<Self> {
        let terminal = Arc::new(Mutex::new(Terminal::new(rows, columns)));
        let session = Session::spawn(program, arguments, rows, columns, Arc::clone(&terminal))?;
        Ok(Self {
            terminal,
            title: session.initial_title.clone(),
            session,
            text: TextInput::default(),
            wheel: 0,
        })
    }
}

struct State {
    tabs: Vec<Tab>,
    active: usize,
    program: PathBuf,
    arguments: Vec<OsString>,
    layout: Layout,
    dpi: u32,
    hover: Option<Control>,
    pressed: Option<Control>,
    gesture: Option<mouse::Gesture>,
    title_dirty: bool,
    fonts: Fonts,
    font_settings: FontSettings,
    settings_file: Option<ConfigFile<FontSettings>>,
    keymap: Keymap,
    keymap_file: Option<ConfigFile<Keymap>>,
    symbols: Symbols,
    focused: bool,
    cursor_visible: bool,
    cursor_tick: Instant,
    size: (u16, u16),
    client_size: (i32, i32),
    error: Option<String>,
    status: i32,
}

impl State {
    fn new(
        program: &Path,
        arguments: &[OsString],
        dpi: u32,
        config_dir: Option<PathBuf>,
    ) -> io::Result<Self> {
        let symbols = Symbols::load()?;
        let settings_file = config_dir
            .as_ref()
            .map(|dir| ConfigFile::open(dir.join("settings.json")));
        let keymap_file: Option<ConfigFile<Keymap>> =
            config_dir.map(|dir| ConfigFile::open(dir.join("keymap.json")));
        let (keymap, error) = match keymap_file.as_ref().map(|file| &file.current) {
            Some(Ok(map)) => (map.clone(), None),
            Some(Err(error)) => (Keymap::default(), Some(error.clone())),
            None => (Keymap::default(), None),
        };
        let initial = settings_file.as_ref().map(|file| file.current.clone());
        let font_settings = FontSettings::default();
        let fonts = Fonts::new(dpi, &font_settings)?;
        let size = (30, 100);
        let mut state = Self {
            tabs: vec![Tab::spawn(program, arguments, size)?],
            active: 0,
            program: program.to_owned(),
            arguments: arguments.to_owned(),
            layout: Layout::new(dpi, 0, 1, 0, 0),
            dpi,
            hover: None,
            pressed: None,
            gesture: None,
            title_dirty: true,
            fonts,
            font_settings,
            settings_file,
            keymap,
            keymap_file,
            symbols,
            focused: false,
            cursor_visible: true,
            cursor_tick: Instant::now(),
            size,
            client_size: (0, 0),
            error,
            status: 0,
        };
        if let Some(initial) = initial {
            state.apply_font(initial);
        }
        Ok(state)
    }

    fn current(&self) -> &Tab {
        &self.tabs[self.active]
    }

    fn current_mut(&mut self) -> &mut Tab {
        &mut self.tabs[self.active]
    }

    fn relayout(&mut self, width: i32) {
        self.layout = Layout::new(
            self.dpi,
            width,
            self.tabs.len(),
            self.active,
            self.layout.first,
        );
        self.hover = None;
        self.pressed = None;
        self.gesture = None;
    }

    fn select(&mut self, index: usize) {
        self.active = index;
        self.title_dirty = true;
        self.cursor_visible = true;
        self.cursor_tick = Instant::now();
        self.relayout(self.layout.width);
    }

    fn close(&mut self, index: usize) {
        self.tabs.remove(index);
        if !self.tabs.is_empty() {
            let active = self.active - usize::from(index < self.active);
            self.select(active.min(self.tabs.len() - 1));
        }
    }

    fn activate(&mut self, control: Control) -> io::Result<()> {
        match control {
            Control::Tab(index) => self.select(index),
            Control::CloseTab(index) => self.close(index),
            Control::New => {
                let tab = Tab::spawn(&self.program, &self.arguments, self.size)?;
                self.tabs.push(tab);
                self.select(self.tabs.len() - 1);
            }
            Control::Previous if self.layout.first > 0 => self.select(self.layout.first - 1),
            Control::Next if self.layout.end < self.tabs.len() => self.select(self.layout.end),
            _ => {}
        }
        Ok(())
    }

    fn resize(&mut self, width: i32, height: i32) {
        self.client_size = (width, height);
        if width <= 0 || height <= 0 {
            return;
        } // A minimized window has no grid.
        self.relayout(width);
        let padding = px(self.dpi, PADDING);
        let columns = ((width - 2 * padding) / self.fonts.width).clamp(1, 1000) as u16;
        let rows =
            ((height - self.layout.height - 2 * padding) / self.fonts.height).clamp(1, 500) as u16;
        if self.size != (rows, columns) {
            self.size = (rows, columns);
            for tab in &self.tabs {
                tab.session.resize(rows, columns);
            }
        }
    }

    fn send(&mut self, bytes: Vec<u8>) {
        if bytes.is_empty() {
            return;
        }
        let tab = &self.tabs[self.active];
        tab.terminal.lock().unwrap().follow_output();
        self.cursor_visible = true;
        self.cursor_tick = Instant::now();
        // The exit event owns shutdown if the input pipe closes first.
        let _ = tab.session.write(bytes);
    }

    fn shortcut(&mut self, code: u16, modifiers: Modifiers, repeat: bool, hwnd: HWND) -> bool {
        let Some(action) = self.keymap.action(code, modifiers) else {
            return false;
        };
        match action {
            Action::NewTab | Action::CloseTab | Action::PreviousTab | Action::NextTab if repeat => {
            }
            Action::NewTab => {
                if let Err(error) = self.activate(Control::New) {
                    self.error = Some(format!("Could not open a tab.\n\n{error}"));
                }
            }
            Action::CloseTab => self.close(self.active),
            Action::PreviousTab => {
                self.select((self.active + self.tabs.len() - 1) % self.tabs.len())
            }
            Action::NextTab => self.select((self.active + 1) % self.tabs.len()),
            Action::WordLeft | Action::WordRight => {
                let key = if action == Action::WordLeft {
                    Key::Left
                } else {
                    Key::Right
                };
                let application_cursor = self
                    .current()
                    .terminal
                    .lock()
                    .unwrap()
                    .screen()
                    .application_cursor();
                self.send(input::key(
                    key,
                    Modifiers {
                        alt: true,
                        ..Modifiers::default()
                    },
                    application_cursor,
                ));
            }
            Action::Paste => {
                if let Some(text) = clipboard_text(hwnd) {
                    let bracketed = self
                        .current()
                        .terminal
                        .lock()
                        .unwrap()
                        .screen()
                        .bracketed_paste();
                    self.send(input::paste(&text, bracketed));
                }
            }
            Action::Copy => {
                let text = self.current().terminal.lock().unwrap().copy_contents();
                copy_text(hwnd, &text);
            }
            Action::ScrollPageUp | Action::ScrollPageDown => {
                let lines = i32::from(self.size.0).saturating_sub(1);
                self.current()
                    .terminal
                    .lock()
                    .unwrap()
                    .scroll(if action == Action::ScrollPageUp {
                        lines
                    } else {
                        -lines
                    });
            }
        }
        true
    }

    fn terminal_key(
        &mut self,
        code: u16,
        lparam: isize,
        down: bool,
        keyboard: &KeyboardState,
    ) -> bool {
        let modifiers = keyboard.modifiers();
        if down
            && code == VK_Z
            && modifiers.control
            && !modifiers.alt
            && !modifiers.shift
            && self.current().session.stop_foreground().unwrap_or(false)
        {
            return true;
        }
        let terminal = self.current().terminal.lock().unwrap();
        let bytes = if terminal.win32_input_mode() {
            keyboard.encode(code, lparam, down)
        } else if down
            && let Some(bytes) =
                keyboard::vt_key(code, modifiers, terminal.screen().application_cursor())
        {
            bytes.repeat(usize::from((lparam as u16).max(1)))
        } else {
            return false;
        };
        drop(terminal);
        if down && !matches!(code, VK_SHIFT | VK_CONTROL | VK_MENU) {
            self.send(bytes);
        } else {
            // Modifier and release events must not move the scrollback viewport.
            let _ = self.current().session.write(bytes);
        }
        true
    }

    fn tick(&mut self) -> bool {
        if let Some(update) = self.keymap_file.as_mut().and_then(ConfigFile::poll) {
            match update {
                Ok(map) => self.keymap = map,
                Err(error) => self.error = Some(error),
            }
        }
        let update = self.settings_file.as_mut().and_then(ConfigFile::poll);
        let mut redraw = update.is_some_and(|settings| self.apply_font(settings));
        for index in (0..self.tabs.len()).rev() {
            let tab = &mut self.tabs[index];
            let mut exit = None;
            while let Some(event) = tab.session.try_event() {
                match event {
                    Event::Resized(_, _) => {}
                    Event::Exited(status) => {
                        exit = Some(status);
                        break;
                    }
                    Event::Error(error) => {
                        self.error = Some(format!("{}: {error}", tab.title));
                        exit = Some(1);
                        break;
                    }
                }
            }
            if let Some(status) = exit {
                self.close(index);
                if self.tabs.is_empty() {
                    self.status = status;
                }
                redraw = true;
                continue;
            }
            let mut terminal = tab.terminal.lock().unwrap();
            if index == self.active {
                redraw |= terminal.take_dirty();
            }
            if let Some(title) = terminal.take_title() {
                let title = if title.is_empty() {
                    tab.session.initial_title.clone()
                } else {
                    title
                };
                if tab.title != title {
                    tab.title = title;
                    self.title_dirty |= index == self.active;
                    redraw = true;
                }
            }
        }
        if self.focused && self.cursor_tick.elapsed() >= Duration::from_millis(500) {
            self.cursor_tick = Instant::now();
            self.cursor_visible = !self.cursor_visible;
            redraw = true;
        }
        redraw
    }

    fn apply_font(&mut self, update: Result<FontSettings, String>) -> bool {
        let settings = match update {
            Ok(settings) if settings == self.font_settings => return false,
            Ok(settings) => settings,
            Err(error) => {
                self.error = Some(error);
                return false;
            }
        };
        match Fonts::new(self.dpi, &settings) {
            Ok(fonts) => {
                self.fonts = fonts;
                self.font_settings = settings;
                self.resize(self.client_size.0, self.client_size.1);
                true
            }
            Err(error) => {
                self.error = Some(format!("Could not apply the terminal font.\n\n{error}"));
                false
            }
        }
    }

    fn paint(&self, hwnd: HWND) {
        // SAFETY: all GDI objects below are created, selected, restored, and
        // released on this window's thread. Paint is always paired with EndPaint.
        unsafe {
            let mut paint = PAINTSTRUCT::default();
            let target = BeginPaint(hwnd, &mut paint);
            if target.is_null() {
                return;
            }
            let mut client = RECT::default();
            GetClientRect(hwnd, &mut client);
            let memory = CreateCompatibleDC(target);
            let bitmap = CreateCompatibleBitmap(target, client.right.max(1), client.bottom.max(1));
            if memory.is_null() || bitmap.is_null() {
                if !bitmap.is_null() {
                    DeleteObject(bitmap);
                }
                if !memory.is_null() {
                    DeleteDC(memory);
                }
                EndPaint(hwnd, &paint);
                return;
            }
            let previous_bitmap = SelectObject(memory, bitmap);
            let previous_font = SelectObject(memory, self.fonts.text(0));
            let background = CreateSolidBrush(DEFAULT_BACKGROUND);
            FillRect(memory, &client, background);
            DeleteObject(background);

            if !self.tabs.is_empty() {
                self.layout.paint(memory, self, IsZoomed(hwnd) != 0);
                if paint.rcPaint.bottom > self.layout.height {
                    self.paint_terminal(memory);
                }
            }
            BitBlt(
                target,
                0,
                0,
                client.right,
                client.bottom,
                memory,
                0,
                0,
                SRCCOPY,
            );
            SelectObject(memory, previous_font);
            SelectObject(memory, previous_bitmap);
            DeleteObject(bitmap);
            DeleteDC(memory);
            EndPaint(hwnd, &paint);
        }
    }

    fn paint_terminal(&self, memory: HDC) {
        let padding = px(self.dpi, PADDING);
        let origin_y = self.layout.height + padding;
        // SAFETY: the caller owns the back buffer and restores its font after drawing.
        unsafe {
            let terminal = self.current().terminal.lock().unwrap();
            let screen = terminal.screen();
            let (rows, columns) = screen.size();
            let mut segment_background = None;
            for row in 0..rows {
                let mut cells = 0..columns;
                while let Some(column) = cells.next() {
                    let Some(cell) = screen.cell(row, column) else {
                        continue;
                    };
                    if cell.is_wide_continuation() {
                        continue;
                    }
                    let (foreground, background) = cell_colors(cell);
                    if segment_background != Some(background) {
                        segment_background = None;
                    }
                    let contents = cell.contents();
                    let x = padding + i32::from(column) * self.fonts.width;
                    let y = origin_y + i32::from(row) * self.fonts.height;
                    if contents == "\u{e0b0}" {
                        segment_background =
                            (background != DEFAULT_BACKGROUND).then_some(background);
                        let rect = RECT {
                            left: x,
                            top: y,
                            right: x + self.fonts.width,
                            bottom: y + self.fonts.height,
                        };
                        paint_separator(memory, rect, foreground, background);
                        continue;
                    }
                    let symbol = self.symbols.glyph(contents);
                    let spaced = symbol.is_some()
                        && !cell.is_wide()
                        && can_share_symbol_space(screen.cell(row, column + 1), background);
                    if spaced {
                        // Paint the blank's background together with the icon so it
                        // cannot erase the right edge on the next iteration.
                        cells.next();
                    }
                    let wide = spaced || cell.is_wide();
                    // A colored segment with padding on both sides has room to
                    // center the icon on its own column, as in a Powerline prompt.
                    let padded = spaced
                        && background != DEFAULT_BACKGROUND
                        && column > 0
                        && can_share_symbol_space(screen.cell(row, column - 1), background);
                    let width = self.fonts.width * if wide { 2 } else { 1 };
                    let rect = RECT {
                        left: if padded { x - self.fonts.width / 2 } else { x },
                        top: y,
                        right: x + width,
                        bottom: y + self.fonts.height,
                    };
                    SetTextColor(memory, foreground);
                    SetBkColor(memory, background);
                    let index = usize::from(cell.bold())
                        + 2 * usize::from(cell.italic())
                        + 4 * usize::from(cell.underline());
                    // Draw the actual glyph index. GDI's UTF-16 fallback can
                    // split supplementary-plane icons such as the Downloads folder.
                    let units;
                    let (font, text, flags, dx, dy) = if let Some(ref glyph) = symbol {
                        let size = if padded {
                            &self.fonts.padded_symbol
                        } else {
                            self.fonts.symbol(wide)
                        };
                        let (mut dx, mut dy) = size.offset(*glyph);
                        if padded && contents == "\u{f17a}" {
                            // Lower the Windows mark by an eighth of the spare
                            // height around its square em, rounded to a pixel.
                            dy += ((self.fonts.height - size.advance).max(0) + 4) / 8;
                        } else if padded
                            && segment_background.is_some()
                            && contents == "\u{f07c}"
                            && screen.cell(row, column + 2).is_some_and(|next| {
                                cell_colors(next).1 == background
                                    && self.symbols.glyph(next.contents()).is_none()
                            })
                        {
                            // Bring the folder toward its path. Adjacent icons share the blank.
                            dx += self.fonts.width / 4;
                        }
                        // In listings, leave a full blank cell before the name.
                        if !padded
                            && wide
                            && can_share_symbol_space(
                                screen.cell(row, column.saturating_add(2)),
                                background,
                            )
                        {
                            dx = (width - size.advance).max(0);
                        }
                        (
                            size.handle(),
                            std::slice::from_ref(glyph),
                            ETO_GLYPH_INDEX,
                            dx,
                            dy,
                        )
                    } else {
                        units = if contents.is_empty() {
                            vec![32]
                        } else {
                            OsStr::new(contents).encode_wide().collect()
                        };
                        let dy = if segment_background.is_some() {
                            self.fonts.segment_text_y
                        } else {
                            0
                        };
                        (self.fonts.text(index), units.as_slice(), 0, 0, dy)
                    };
                    SelectObject(memory, font);
                    ExtTextOutW(
                        memory,
                        x + dx,
                        y + dy,
                        ETO_OPAQUE | ETO_CLIPPED | flags,
                        &rect,
                        text.as_ptr(),
                        text.len() as u32,
                        ptr::null(),
                    );
                }
                if !screen.row_wrapped(row) {
                    segment_background = None;
                }
                if let Some(cols) = terminal.selected_columns(row) {
                    let y = origin_y + i32::from(row) * self.fonts.height;
                    let rect = RECT {
                        left: padding + i32::from(cols.start) * self.fonts.width,
                        right: padding + i32::from(cols.end) * self.fonts.width,
                        top: y,
                        bottom: y + self.fonts.height,
                    };
                    InvertRect(memory, &rect);
                }
            }
            if self.focused
                && self.cursor_visible
                && !screen.hide_cursor()
                && screen.scrollback() == 0
            {
                let (row, column) = screen.cursor_position();
                if row < rows && column < columns {
                    let x = padding + i32::from(column) * self.fonts.width;
                    let y = origin_y + i32::from(row) * self.fonts.height;
                    let cursor_width = (self.fonts.width / 6).max(1);
                    let rect = RECT {
                        left: x,
                        top: y + self.fonts.cursor_y.start,
                        right: x + cursor_width,
                        bottom: y + self.fonts.cursor_y.end,
                    };
                    let brush = CreateSolidBrush(DEFAULT_FOREGROUND);
                    FillRect(memory, &rect, brush);
                    DeleteObject(brush);
                }
            }
        }
    }
}

fn point(lparam: LPARAM) -> POINT {
    POINT {
        x: lparam as i16 as i32,
        y: (lparam >> 16) as i16 as i32,
    }
}

// No State borrow may span a call that pumps messages (including error dialogs).
unsafe fn refresh(hwnd: HWND, state: *mut State, redraw: bool) {
    unsafe {
        if (*state).tabs.is_empty() {
            DestroyWindow(hwnd);
            return;
        }
        if std::mem::take(&mut (*state).title_dirty) {
            let tab = (*state).current();
            let title = if tab.title == "winsh" {
                wide("winsh")
            } else {
                wide(&format!("{} — winsh", tab.title))
            };
            SetWindowTextW(hwnd, title.as_ptr());
        }
        if redraw {
            InvalidateRect(hwnd, ptr::null(), 0);
        }
        if let Some(error) = (*state).error.take() {
            window_message(hwnd, &error, true);
        }
    }
}

unsafe fn activate(hwnd: HWND, state: *mut State, control: Control) {
    unsafe {
        if let Err(error) = (*state).activate(control) {
            (*state).error = Some(format!("Could not open a tab.\n\n{error}"));
        }
        refresh(hwnd, state, true);
    }
}

fn invalidate_bar(hwnd: HWND, layout: &Layout) {
    let bounds = RECT {
        left: 0,
        top: 0,
        right: layout.width,
        bottom: layout.height,
    };
    // SAFETY: the RECT is valid for this call on the live window.
    unsafe { InvalidateRect(hwnd, &bounds, 0) };
}

// Only the messages that need State borrow its pointer. In particular, default
// handling and DestroyWindow must not create an overlapping mutable reference
// when Windows synchronously dispatches another message.
unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // SAFETY: Windows supplies message-specific pointers. WM_NCCREATE receives
    // our boxed State; it outlives the window. WM_NCDESTROY clears its borrowed copy.
    unsafe {
        if message == WM_NCCREATE {
            let create = &*(lparam as *const CREATESTRUCTW);
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize);
            return 1;
        }
        let state = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut State;
        if message == WM_NCDESTROY {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            return DefWindowProcW(hwnd, message, wparam, lparam);
        }
        match message {
            WM_NCCALCSIZE if !state.is_null() => {
                // Both forms start with a RECT. Keep Windows' side and bottom
                // borders, and its offscreen resize margin when maximized.
                let rect = lparam as *mut RECT;
                let top = (*rect).top;
                DefWindowProcW(hwnd, message, wparam, lparam);
                (*rect).top = if IsZoomed(hwnd) != 0 {
                    (*rect).top - GetSystemMetricsForDpi(SM_CYCAPTION, (*state).dpi)
                } else {
                    top
                };
                0
            }
            WM_NCHITTEST if !state.is_null() => {
                let mut pt = point(lparam);
                ScreenToClient(hwnd, &mut pt);
                let layout = &(*state).layout;
                if pt.x < 0 || pt.x >= layout.width || pt.y < 0 || pt.y >= layout.height {
                    return DefWindowProcW(hwnd, message, wparam, lparam);
                }
                if pt.y < px((*state).dpi, 5) && IsZoomed(hwnd) == 0 {
                    let edge = px((*state).dpi, 9);
                    return (if pt.x < edge {
                        HTTOPLEFT
                    } else if pt.x >= layout.width - edge {
                        HTTOPRIGHT
                    } else {
                        HTTOP
                    }) as LRESULT;
                }
                match layout.hit(pt.x, pt.y) {
                    Some(Control::Minimize) => HTMINBUTTON as LRESULT,
                    Some(Control::Maximize) => HTMAXBUTTON as LRESULT,
                    Some(Control::CloseWindow) => HTCLOSE as LRESULT,
                    Some(_) => HTCLIENT as LRESULT,
                    None => HTCAPTION as LRESULT,
                }
            }
            WM_GETMINMAXINFO if !state.is_null() => {
                let result = DefWindowProcW(hwnd, message, wparam, lparam);
                let minimum = &mut (*(lparam as *mut MINMAXINFO)).ptMinTrackSize;
                minimum.x = minimum.x.max(px((*state).dpi, 420));
                minimum.y = minimum.y.max(px((*state).dpi, 150));
                result
            }
            WM_NCACTIVATE => DefWindowProcW(hwnd, message, wparam, -1),
            WM_CLOSE => {
                DestroyWindow(hwnd);
                0
            }
            WM_DESTROY => {
                KillTimer(hwnd, 1);
                PostQuitMessage(0);
                0
            }
            WM_ERASEBKGND => 1,
            WM_SIZE if !state.is_null() => {
                (*state).resize(
                    (lparam as u32 & 0xffff) as i32,
                    ((lparam as u32 >> 16) & 0xffff) as i32,
                );
                InvalidateRect(hwnd, ptr::null(), 0);
                0
            }
            WM_DPICHANGED if !state.is_null() => {
                let dpi = (wparam as u32 & 0xffff).max(96);
                if let Ok(fonts) = Fonts::new(dpi, &(*state).font_settings) {
                    (*state).fonts = fonts;
                }
                (*state).dpi = dpi;
                let rect = &*(lparam as *const RECT);
                SetWindowPos(
                    hwnd,
                    ptr::null_mut(),
                    rect.left,
                    rect.top,
                    rect.right - rect.left,
                    rect.bottom - rect.top,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
                0
            }
            WM_SETFOCUS | WM_KILLFOCUS if !state.is_null() => {
                (*state).focused = message == WM_SETFOCUS;
                if message == WM_KILLFOCUS && GetCapture() == hwnd {
                    ReleaseCapture();
                }
                InvalidateRect(hwnd, ptr::null(), 0);
                0
            }
            WM_PAINT if !state.is_null() => {
                (*state).paint(hwnd);
                0
            }
            WM_TIMER if !state.is_null() => {
                let redraw = (*state).tick();
                refresh(hwnd, state, redraw);
                0
            }
            WM_CHAR | WM_SYSCHAR if !state.is_null() => {
                let unit = wparam as u16;
                if message == WM_SYSCHAR && unit == 32 {
                    return DefWindowProcW(hwnd, message, wparam, lparam);
                }
                let tab = (*state).current_mut();
                let mut text = tab.text.push(if unit == 8 { 127 } else { unit });
                if message == WM_SYSCHAR && !text.is_empty() {
                    text.insert(0, '\x1b');
                }
                (*state).send(text.into_bytes());
                0
            }
            WM_UNICHAR if !state.is_null() => {
                if wparam == 0xffff {
                    return 1;
                }
                if let Some(ch) = char::from_u32(wparam as u32) {
                    (*state).send(ch.to_string().into_bytes());
                }
                0
            }
            WM_MOUSEWHEEL if !state.is_null() => {
                let tab = (*state).current_mut();
                tab.wheel += (wparam >> 16) as i16 as i32;
                let lines = tab.wheel / 120 * 3;
                tab.wheel %= 120;
                tab.terminal.lock().unwrap().scroll(lines);
                0
            }
            WM_SETCURSOR if !state.is_null() && lparam as u16 == HTCLIENT as u16 => {
                let mut pt = POINT::default();
                GetCursorPos(&mut pt);
                ScreenToClient(hwnd, &mut pt);
                let cursor = if pt.y < (*state).layout.height {
                    IDC_ARROW
                } else {
                    IDC_IBEAM
                };
                SetCursor(LoadCursorW(ptr::null_mut(), cursor));
                1
            }
            WM_MOUSEMOVE | WM_NCMOUSEMOVE if !state.is_null() => {
                let nc = message == WM_NCMOUSEMOVE;
                let mut pt = point(lparam);
                if nc {
                    ScreenToClient(hwnd, &mut pt);
                }
                if (*state).gesture.is_some() {
                    (*state).update_gesture(pt, false);
                    return 0;
                }
                let hit = (*state).layout.hit(pt.x, pt.y);
                if (*state).hover != hit {
                    (*state).hover = hit;
                    invalidate_bar(hwnd, &(*state).layout);
                }
                if hit.is_some() {
                    let mut track = TRACKMOUSEEVENT {
                        cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: TME_LEAVE | if nc { TME_NONCLIENT } else { 0 },
                        hwndTrack: hwnd,
                        dwHoverTime: 0,
                    };
                    TrackMouseEvent(&mut track);
                }
                DefWindowProcW(hwnd, message, wparam, lparam)
            }
            WM_MOUSELEAVE | WM_NCMOUSELEAVE if !state.is_null() => {
                if (*state).hover.take().is_some() {
                    invalidate_bar(hwnd, &(*state).layout);
                }
                DefWindowProcW(hwnd, message, wparam, lparam)
            }
            WM_LBUTTONDOWN | WM_LBUTTONDBLCLK if !state.is_null() => {
                let pt = point(lparam);
                (*state).pressed = (*state).layout.hit(pt.x, pt.y);
                if (*state).pressed.is_some() {
                    SetCapture(hwnd);
                    invalidate_bar(hwnd, &(*state).layout);
                } else if (*state).begin_gesture(pt) {
                    SetCapture(hwnd);
                }
                0
            }
            WM_LBUTTONUP if !state.is_null() => {
                let pt = point(lparam);
                let pressed = (*state).pressed.take();
                let hit = (*state).layout.hit(pt.x, pt.y);
                (*state).update_gesture(pt, true);
                if GetCapture() == hwnd {
                    ReleaseCapture();
                }
                if let Some(control) = pressed.filter(|&c| Some(c) == hit) {
                    activate(hwnd, state, control);
                }
                0
            }
            WM_CAPTURECHANGED if !state.is_null() => {
                (*state).pressed = None;
                (*state).gesture = None;
                invalidate_bar(hwnd, &(*state).layout);
                0
            }
            _ => DefWindowProcW(hwnd, message, wparam, lparam),
        }
    }
}

fn wide(text: &str) -> Vec<u16> {
    OsStr::new(text)
        .encode_wide()
        .filter(|unit| *unit != 0)
        .chain([0])
        .collect()
}

struct Clipboard;
impl Drop for Clipboard {
    fn drop(&mut self) {
        // SAFETY: constructed only after OpenClipboard succeeds on this thread.
        unsafe { CloseClipboard() };
    }
}

fn clipboard_text(hwnd: HWND) -> Option<String> {
    // SAFETY: lock the clipboard, bound reads to the global allocation, and
    // release the lock before CloseClipboard. Clipboard data remains borrowed.
    unsafe {
        if OpenClipboard(hwnd) == 0 {
            return None;
        }
        let _clipboard = Clipboard;
        let handle = GetClipboardData(CF_UNICODETEXT);
        if handle.is_null() {
            return None;
        }
        let count = (GlobalSize(handle) / 2).min(512 * 1024);
        let pointer = GlobalLock(handle) as *const u16;
        if pointer.is_null() {
            return None;
        }
        let units = std::slice::from_raw_parts(pointer, count);
        let length = units.iter().position(|unit| *unit == 0).unwrap_or(count);
        let result = String::from_utf16_lossy(&units[..length]);
        GlobalUnlock(handle);
        Some(result)
    }
}

fn copy_text(hwnd: HWND, text: &str) {
    let units = wide(&text.replace('\n', "\r\n"));
    // SAFETY: allocate and initialize clipboard-owned movable memory. Ownership
    // transfers only if SetClipboardData succeeds; otherwise free it here.
    unsafe {
        if OpenClipboard(hwnd) == 0 {
            return;
        }
        let _clipboard = Clipboard;
        let handle = GlobalAlloc(GMEM_MOVEABLE, units.len() * 2);
        if handle.is_null() {
            return;
        }
        let pointer = GlobalLock(handle) as *mut u16;
        if pointer.is_null() {
            GlobalFree(handle);
            return;
        }
        ptr::copy_nonoverlapping(units.as_ptr(), pointer, units.len());
        GlobalUnlock(handle);
        if EmptyClipboard() == 0 || SetClipboardData(CF_UNICODETEXT, handle).is_null() {
            GlobalFree(handle);
        }
    }
}

const fn rgb(red: u8, green: u8, blue: u8) -> u32 {
    red as u32 | ((green as u32) << 8) | ((blue as u32) << 16)
}

// A Powerline join meets both neighboring backgrounds edge to edge. The
// inset used for ordinary icons would leave gaps above and below the wedge.
fn paint_separator(dc: HDC, rect: RECT, foreground: u32, background: u32) {
    // SAFETY: the paint DC is live and the stock brush and pen are borrowed.
    unsafe {
        let brush = GetStockObject(DC_BRUSH);
        let previous_brush = SelectObject(dc, brush);
        let previous_pen = SelectObject(dc, GetStockObject(NULL_PEN));
        let previous_color = SetDCBrushColor(dc, background);
        FillRect(dc, &rect, brush);
        SetDCBrushColor(dc, foreground);
        let corners = [
            POINT {
                x: rect.left,
                y: rect.top,
            },
            POINT {
                x: rect.right,
                y: rect.top + (rect.bottom - rect.top) / 2,
            },
            POINT {
                x: rect.left,
                y: rect.bottom,
            },
        ];
        Polygon(dc, corners.as_ptr(), corners.len() as i32);
        SetDCBrushColor(dc, previous_color);
        SelectObject(dc, previous_pen);
        SelectObject(dc, previous_brush);
    }
}

fn cell_colors(cell: &vt100::Cell) -> (u32, u32) {
    let fg = color(cell.fgcolor(), DEFAULT_FOREGROUND);
    let bg = color(cell.bgcolor(), DEFAULT_BACKGROUND);
    if cell.inverse() { (bg, fg) } else { (fg, bg) }
}

fn can_share_symbol_space(next: Option<&vt100::Cell>, background: u32) -> bool {
    next.is_some_and(|cell| {
        matches!(cell.contents(), "" | " ")
            && !cell.is_wide_continuation()
            && !cell.underline()
            && cell_colors(cell).1 == background
    })
}

fn color(color: vt100::Color, default: u32) -> u32 {
    match color {
        vt100::Color::Default => default,
        vt100::Color::Rgb(red, green, blue) => rgb(red, green, blue),
        vt100::Color::Idx(index) => match index {
            0..=15 => [
                rgb(20, 23, 29),
                rgb(224, 108, 117),
                rgb(152, 195, 121),
                rgb(229, 192, 123),
                rgb(97, 175, 239),
                rgb(198, 120, 221),
                rgb(86, 182, 194),
                rgb(171, 178, 191),
                rgb(92, 99, 112),
                rgb(255, 128, 137),
                rgb(178, 221, 147),
                rgb(255, 218, 149),
                rgb(123, 201, 255),
                rgb(224, 146, 247),
                rgb(112, 208, 220),
                rgb(240, 243, 249),
            ][index as usize],
            16..=231 => {
                let index = index - 16;
                let values = [0, 95, 135, 175, 215, 255];
                rgb(
                    values[(index / 36) as usize],
                    values[((index / 6) % 6) as usize],
                    values[(index % 6) as usize],
                )
            }
            232..=255 => {
                let level = 8 + (index - 232) * 10;
                rgb(level, level, level)
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn console_child_distinguishes_control_letters_and_modified_enter() {
        let program = std::env::current_exe().unwrap();
        let args = [
            "--ignored",
            "--exact",
            "window::tests::read_console_keys",
            "--nocapture",
        ]
        .map(OsString::from);
        let mut state = State::new(&program, &args, 96, None).unwrap();
        let terminal = Arc::clone(&state.current().terminal);
        let wait_for = |expected: &str| {
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                let contents = terminal.lock().unwrap().screen().contents();
                if contents.contains(expected) {
                    break;
                }
                assert!(
                    Instant::now() < deadline,
                    "waiting for {expected:?}: {contents}"
                );
                std::thread::sleep(Duration::from_millis(10));
            }
        };
        wait_for("reading keys");
        assert!(terminal.lock().unwrap().win32_input_mode());
        for (code, scan, modifier) in [
            (VK_J, 0x24, VK_CONTROL),
            (VK_RETURN, 0x1c, VK_SHIFT),
            (VK_X, 0x2d, VK_CONTROL),
            (VK_H, 0x23, VK_CONTROL),
            (VK_I, 0x17, VK_CONTROL),
            (VK_RETURN, 0x1c, 0),
        ] {
            let mut keys = KeyboardState([0; 256]);
            if modifier != 0 {
                keys.0[usize::from(modifier)] = 0x80;
            }
            if modifier == VK_CONTROL {
                keys.0[usize::from(VK_LCONTROL)] = 0x80;
            }
            assert!(state.terminal_key(code, (scan << 16) | 1, true, &keys));
            assert!(state.terminal_key(code, (scan << 16) | 1, false, &keys));
        }
        wait_for("keys received");
    }

    #[test]
    #[ignore = "native keyboard input fixture, launched inside ConPTY"]
    fn read_console_keys() {
        use windows_sys::Win32::System::Console::*;
        // SAFETY: this child is attached to the test's pseudoconsole. The buffer
        // holds one input record and the union is read only for keyboard events.
        unsafe {
            let handle = GetStdHandle(STD_INPUT_HANDLE);
            assert_ne!(SetConsoleMode(handle, 0), 0);
            println!("reading keys");
            for (code, unit, flags) in [
                (VK_J, 10, LEFT_CTRL_PRESSED),
                (VK_RETURN, 13, SHIFT_PRESSED),
                (VK_X, 24, LEFT_CTRL_PRESSED),
                (VK_H, 8, LEFT_CTRL_PRESSED),
                (VK_I, 9, LEFT_CTRL_PRESSED),
                (VK_RETURN, 13, 0),
            ] {
                for down in [true, false] {
                    let mut record = INPUT_RECORD::default();
                    let mut count = 0;
                    loop {
                        assert_ne!(ReadConsoleInputW(handle, &mut record, 1, &mut count), 0);
                        if count == 1 && u32::from(record.EventType) == KEY_EVENT {
                            break;
                        }
                    }
                    let key = record.Event.KeyEvent;
                    assert_eq!(
                        (
                            key.wVirtualKeyCode,
                            key.uChar.UnicodeChar,
                            key.dwControlKeyState,
                            key.bKeyDown != 0
                        ),
                        (code, unit, flags, down)
                    );
                }
            }
            println!("keys received");
        }
    }

    #[test]
    fn keymap_drives_tab_switching_and_reloads_in_open_tabs() {
        let dir = tempfile::tempdir().unwrap();
        let program = PathBuf::from(std::env::var_os("ComSpec").unwrap());
        let mut state = State::new(
            &program,
            &["/d".into(), "/q".into()],
            96,
            Some(dir.path().into()),
        )
        .unwrap();
        let path = dir.path().join("keymap.json");
        assert!(path.is_file());
        let ctrl = Modifiers {
            control: true,
            ..Modifiers::default()
        };
        let alt = Modifiers {
            alt: true,
            ..Modifiers::default()
        };
        let hwnd = ptr::null_mut();
        assert!(state.shortcut(u16::from(b'T'), ctrl, false, hwnd));
        assert_eq!((state.tabs.len(), state.active), (2, 1));
        state.activate(Control::New).unwrap();
        assert!(state.shortcut(VK_RIGHT, ctrl, false, hwnd));
        assert_eq!(state.active, 0);
        assert!(state.shortcut(VK_RIGHT, ctrl, true, hwnd));
        assert_eq!(state.active, 0);
        assert!(state.shortcut(VK_LEFT, ctrl, false, hwnd));
        assert_eq!(state.active, 2);
        assert_eq!(state.keymap.action(VK_LEFT, alt), Some(Action::WordLeft));
        assert_eq!(state.keymap.action(VK_RIGHT, alt), Some(Action::WordRight));
        assert_eq!(state.keymap.action(VK_LEFT, Modifiers::default()), None);

        let wait_for = |state: &mut State, ready: &dyn Fn(&State) -> bool| {
            let deadline = Instant::now() + Duration::from_secs(5);
            while !ready(state) {
                assert!(
                    Instant::now() < deadline,
                    "timed out waiting for keymap edit"
                );
                state.tick();
                std::thread::sleep(Duration::from_millis(10));
            }
        };
        std::fs::write(&path, r#"{"CONTROL+J":"next_tab", "alt+left":"word_left"}"#).unwrap();
        let j = u16::from(b'J');
        wait_for(&mut state, &|s| {
            s.keymap.action(j, ctrl) == Some(Action::NextTab)
        });
        assert!(!state.shortcut(VK_RIGHT, ctrl, false, hwnd));
        assert!(state.shortcut(j, ctrl, false, hwnd));
        assert_eq!(state.active, 0);
        std::fs::write(&path, r#"{"ctrl+badkey":"close_tab"}"#).unwrap();
        wait_for(&mut state, &|s| s.error.is_some());
        assert!(state.error.take().unwrap().contains("ctrl+badkey"));
        assert!(state.shortcut(j, ctrl, false, hwnd));
        assert_eq!(state.active, 1);
        std::fs::write(&path, "{}").unwrap();
        wait_for(&mut state, &|s| s.keymap.action(j, ctrl).is_none());
        assert_eq!(state.tabs.len(), 3);
    }

    #[test]
    fn saving_font_settings_resizes_live_tabs_and_recovers_from_invalid_edits() {
        use std::io::Write;
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("winsh/settings.json");
        let program = PathBuf::from(std::env::var_os("ComSpec").unwrap());
        let mut state = State::new(
            &program,
            &["/d".into(), "/q".into()],
            96,
            Some(path.parent().unwrap().to_owned()),
        )
        .unwrap();
        assert!(path.is_file());
        assert_eq!(
            state.settings_file.as_ref().unwrap().current,
            Ok(FontSettings::default())
        );
        state.resize(800, 600);
        state.activate(Control::New).unwrap();
        let original_grid = state.size;
        let original_height = state.fonts.height;
        let original_tab = state.current().terminal.clone();

        let wait_for = |state: &mut State, ready: &dyn Fn(&State) -> bool| {
            let deadline = Instant::now() + Duration::from_secs(5);
            while !ready(state) {
                assert!(Instant::now() < deadline, "timed out waiting for font edit");
                state.tick();
                std::thread::sleep(Duration::from_millis(10));
            }
        };
        let mut replacement = tempfile::NamedTempFile::new_in(path.parent().unwrap()).unwrap();
        replacement
            .write_all(br#"{"terminal":{"font_size_px":16}}"#)
            .unwrap();
        replacement.persist(&path).unwrap();
        wait_for(&mut state, &|s| {
            s.font_settings.font_size_px == 16.0
                && s.tabs
                    .iter()
                    .all(|tab| tab.terminal.lock().unwrap().screen().size() == s.size)
        });
        assert!(state.fonts.height > original_height && state.size.0 < original_grid.0);
        assert_eq!(state.font_settings.font_family, "Cascadia Mono");
        assert!(Arc::ptr_eq(&original_tab, &state.current().terminal));

        std::fs::write(&path, r#"{"terminal":{"font_size_px":0}}"#).unwrap();
        wait_for(&mut state, &|s| s.error.is_some());
        assert!(state.error.take().unwrap().contains("font_size_px"));
        assert_eq!(state.font_settings.font_size_px, 16.0);
        std::fs::write(&path, "{").unwrap();
        wait_for(&mut state, &|s| s.error.is_some());
        assert!(state.error.take().unwrap().contains("settings.json"));
        assert_eq!(state.font_settings.font_size_px, 16.0);
        std::fs::write(&path, r#"{"terminal":{"font_family":"Consolas"}}"#).unwrap();
        wait_for(&mut state, &|s| s.font_settings.font_family == "Consolas");
        assert_eq!(state.font_settings.font_size_px, 14.0);
        let reopened = ConfigFile::<FontSettings>::open(path.clone());
        assert_eq!(reopened.current.as_ref().unwrap(), &state.font_settings);
        std::fs::remove_file(path).unwrap();
        wait_for(&mut state, &|s| s.font_settings == FontSettings::default());
        assert_eq!(state.size, original_grid);
    }

    #[test]
    fn background_tab_can_update_and_exit_while_the_selected_shell_keeps_its_state() {
        let program = PathBuf::from(std::env::var_os("ComSpec").unwrap());
        let mut state = State::new(&program, &["/d".into(), "/q".into()], 96, None).unwrap();
        state.send(b"set WINSH_TAB_TEST=first\recho ready\r".to_vec());
        state.activate(Control::New).unwrap();
        state.send(b"if not defined WINSH_TAB_TEST title independent tab\r".to_vec());
        state.select(0);
        let wait = |state: &mut State, ready: fn(&State) -> bool| {
            let deadline = Instant::now() + Duration::from_secs(10);
            while !ready(state) {
                state.tick();
                assert!(state.error.is_none(), "{:?}", state.error);
                assert!(Instant::now() < deadline, "timed out waiting for shell");
                std::thread::sleep(Duration::from_millis(10));
            }
        };
        wait(&mut state, |s| s.tabs[1].title.ends_with("independent tab"));
        state.resize(680, 360);
        wait(&mut state, |s| {
            s.tabs
                .iter()
                .all(|t| t.terminal.lock().unwrap().screen().size() == s.size)
        });
        state.send(b"if \"%WINSH_TAB_TEST%\"==\"first\" title original tab\r".to_vec());
        wait(&mut state, |s| s.current().title.ends_with("original tab"));
        state.select(1);
        state.send(b"exit 19\r".to_vec());
        state.select(0);
        wait(&mut state, |s| s.tabs.len() == 1);
        assert!(state.current().title.ends_with("original tab"));
        assert_eq!(state.status, 0);
        state.send(b"exit 37\r".to_vec());
        wait(&mut state, |s| s.tabs.is_empty());
        assert_eq!(state.status, 37);
    }

    #[test]
    fn prompt_icon_spacing_and_label_baselines() {
        let program = PathBuf::from(std::env::var_os("ComSpec").unwrap());
        let mut state = State::new(&program, &["/d".into(), "/q".into()], 96, None).unwrap();
        let mut terminal = Terminal::new(1, 22);
        terminal.process(
            concat!(
                "\x1b[47;30m \u{f17a} ",
                "\x1b[46m\u{e0b0} \u{f07c} ~H ",
                "\x1b[42m\u{e0b0} \u{f113} \u{e0a0} m\x1b[1mH\x1b[22mg",
                "\x1b[49m\u{e0b0}\x1b[0mH",
            )
            .as_bytes(),
        );
        state.current_mut().terminal = Arc::new(Mutex::new(terminal));
        for dpi in [96, 120, 144, 192] {
            state.dpi = dpi;
            state.fonts = Fonts::new(dpi, &state.font_settings).unwrap();
            let w = state.fonts.width;
            let h = state.fonts.height;
            let x = px(dpi, PADDING);
            let y = state.layout.height + x;
            // SAFETY: select the bitmap into a memory DC only for the duration of the render.
            unsafe {
                let screen = GetDC(ptr::null_mut());
                let dc = CreateCompatibleDC(screen);
                let bmp = CreateCompatibleBitmap(screen, x + 22 * w, y + h);
                ReleaseDC(ptr::null_mut(), screen);
                assert!(!dc.is_null() && !bmp.is_null());
                let previous = SelectObject(dc, bmp);
                state.paint_terminal(dc);
                let bounds = |start: i32, end: i32| {
                    let bg = GetPixel(dc, x + start, y);
                    let (mut left, mut top, mut right, mut bottom) = (end, h, start, 0);
                    for dy in 0..h {
                        for dx in start..end {
                            if GetPixel(dc, x + dx, y + dy) != bg {
                                left = left.min(dx);
                                right = right.max(dx + 1);
                                top = top.min(dy);
                                bottom = bottom.max(dy + 1);
                            }
                        }
                    }
                    assert!(
                        left < right && top < bottom,
                        "missing ink at {start}..{end}"
                    );
                    (left, top, right, bottom)
                };
                for col in [1, 12, 14] {
                    let center = col * w + w / 2;
                    let (left, top, right, bottom) = bounds(center - w, center + w);
                    // Allow one pixel for rasterization and rounding to the grid.
                    assert!(
                        (left + right - (2 * col + 1) * w).abs() <= 2,
                        "column {col} at {dpi} DPI"
                    );
                    if col == 1 {
                        assert!(right - left > w + w / 2, "OS badge should use its padding");
                        assert!(
                            (h..=h + 3).contains(&(top + bottom)),
                            "Windows logo at {dpi} DPI"
                        );
                    } else {
                        assert!((top + bottom - h).abs() <= 2, "column {col} at {dpi} DPI");
                    }
                }
                let (_, _, folder_right, _) = bounds(4 * w, 7 * w);
                let (tilde_left, _, _, _) = bounds(7 * w, 8 * w);
                assert!(
                    (1..=(w + 1) / 2).contains(&(tilde_left - folder_right)),
                    "folder gap at {dpi} DPI"
                );
                let (_, cap_top, _, cap_bottom) = bounds(8 * w, 9 * w);
                let (_, bold_top, _, bold_bottom) = bounds(17 * w, 18 * w);
                let (_, _, _, descender) = bounds(18 * w, 19 * w);
                let (_, normal_top, _, _) = bounds(20 * w, 21 * w);
                assert!(
                    (cap_top + cap_bottom - h).abs() <= 2,
                    "path label at {dpi} DPI"
                );
                assert!(
                    (bold_top + bold_bottom - h).abs() <= 2,
                    "bold label at {dpi} DPI"
                );
                assert!(cap_bottom < descender && descender < h);
                assert_eq!(cap_top - normal_top, state.fonts.segment_text_y);
                // Selecting only an icon's shared padding cell must highlight
                // that column even though GDI drew it together with the icon.
                let pixel = |col| GetPixel(dc, x + col * w + w / 2, y + h / 2);
                let (icon, padding) = (pixel(1), pixel(2));
                let terminal = &state.current().terminal;
                terminal.lock().unwrap().select((0, 2), (0, 3));
                state.paint_terminal(dc);
                assert_eq!(pixel(1), icon);
                assert_eq!(pixel(2), padding ^ 0x00ff_ffff);
                terminal.lock().unwrap().select((0, 0), (0, 0));
                SelectObject(dc, previous);
                DeleteObject(bmp);
                DeleteDC(dc);
            }
        }
    }

    #[test]
    fn symbol_padding_respects_neighboring_text_and_cell_attributes() {
        for (after, fits) in [
            (" ", true),
            ("", true),
            ("\x1b[34m ", true),
            ("D", false),
            ("\u{f115}", false),
            ("猫", false),
            ("\x1b[4m ", false),
            ("\x1b[44m ", false),
            ("\x1b[7m ", false),
        ] {
            let mut terminal = Terminal::new(1, 4);
            terminal.process(format!("\u{f024d}{after}").as_bytes());
            let screen = terminal.screen();
            assert_eq!(
                can_share_symbol_space(screen.cell(0, 1), DEFAULT_BACKGROUND),
                fits,
                "{after:?}"
            );
            assert!(!can_share_symbol_space(
                screen.cell(0, 4),
                DEFAULT_BACKGROUND
            ));
        }
    }
}
