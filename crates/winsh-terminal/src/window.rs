//! The native window and GDI renderer. All pointers and GDI objects stay on the
//! window thread; the session only shares the terminal grid through a mutex.
use std::{
    ffi::{OsStr, OsString},
    io,
    os::windows::ffi::OsStrExt,
    path::Path,
    ptr,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use windows_sys::{
    Win32::{
        Foundation::{GlobalFree, HWND, LPARAM, LRESULT, RECT, WPARAM},
        Graphics::Gdi::*,
        System::{
            DataExchange::{
                CloseClipboard, EmptyClipboard, GetClipboardData, OpenClipboard, SetClipboardData,
            },
            LibraryLoader::GetModuleHandleW,
            Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock},
        },
        UI::{
            HiDpi::{
                DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForSystem,
                SetProcessDpiAwarenessContext,
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
    let terminal = Arc::new(Mutex::new(Terminal::new(30, 100)));
    let mut state = Box::new(State {
        terminal: Arc::clone(&terminal),
        session: None,
        fonts: Fonts::new(dpi)?,
        text: TextInput::default(),
        focused: false,
        cursor_visible: true,
        cursor_tick: Instant::now(),
        size: (30, 100),
        wheel: 0,
        error: None,
        status: 0,
    });
    // SAFETY: static system cursor, borrowed for the window class lifetime.
    let cursor = unsafe { LoadCursorW(ptr::null_mut(), IDC_IBEAM) };
    let class = WNDCLASSW {
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(window_proc),
        hInstance: instance,
        hCursor: cursor,
        lpszClassName: w!("WinshTerminalWindow"),
        ..Default::default()
    };
    // SAFETY: class fields have valid static pointers, module, and callback.
    if unsafe { RegisterClassW(&class) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let mut rectangle = RECT {
        left: 0,
        top: 0,
        right: 100 * state.fonts.width + 2 * PADDING,
        bottom: 30 * state.fonts.height + 2 * PADDING,
    };
    // SAFETY: initialized rectangle and the style used for CreateWindowExW.
    unsafe { AdjustWindowRectEx(&mut rectangle, WS_OVERLAPPEDWINDOW, 0, 0) };
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
            rectangle.bottom - rectangle.top,
            ptr::null_mut(),
            ptr::null_mut(),
            instance,
            (&mut *state as *mut State).cast(),
        )
    };
    if hwnd.is_null() {
        return Err(io::Error::last_os_error());
    }
    let session = Session::spawn(program, arguments, state.size.0, state.size.1, terminal);
    match session {
        Ok(session) => state.session = Some(session),
        Err(error) => {
            // SAFETY: the window was created on this thread and is still live.
            unsafe { DestroyWindow(hwnd) };
            return Err(error);
        }
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
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
    // Join the session before reporting an error or returning to the caller.
    drop(state.session.take());
    if let Some(error) = state.error.take() {
        return Err(io::Error::other(error));
    }
    Ok(state.status)
}

pub fn message(text: &str, error: bool) {
    let text = wide(text);
    // SAFETY: both strings are terminated and live through this synchronous call.
    unsafe {
        MessageBoxW(
            ptr::null_mut(),
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

struct State {
    terminal: Arc<Mutex<Terminal>>,
    session: Option<Session>,
    fonts: Fonts,
    text: TextInput,
    focused: bool,
    cursor_visible: bool,
    cursor_tick: Instant,
    size: (u16, u16),
    wheel: i32,
    error: Option<String>,
    status: i32,
}

impl State {
    fn resize(&mut self, width: i32, height: i32) {
        if width <= 0 || height <= 0 {
            return;
        } // A minimized window has no grid.
        let columns = ((width - 2 * PADDING) / self.fonts.width).clamp(1, 1000) as u16;
        let rows = ((height - 2 * PADDING) / self.fonts.height).clamp(1, 500) as u16;
        if self.size != (rows, columns) {
            self.size = (rows, columns);
            if let Some(session) = &self.session {
                session.resize(rows, columns);
            } else {
                self.terminal.lock().unwrap().resize(rows, columns);
            }
        }
    }

    fn send(&mut self, bytes: Vec<u8>) {
        if bytes.is_empty() {
            return;
        }
        self.terminal.lock().unwrap().follow_output();
        self.cursor_visible = true;
        self.cursor_tick = Instant::now();
        if let Some(session) = &self.session {
            // Exit may race the final key event; the session's exit event owns
            // shutdown and its status, including when the input pipe closes.
            let _ = session.write(bytes);
        }
    }

    fn key_down(&mut self, code: u16, modifiers: Modifiers, hwnd: HWND) -> bool {
        if (modifiers.control && modifiers.shift && code == b'V' as u16)
            || (modifiers.shift && code == VK_INSERT)
        {
            if let Some(text) = clipboard_text(hwnd) {
                let bracketed = self.terminal.lock().unwrap().screen().bracketed_paste();
                self.send(input::paste(&text, bracketed));
            }
            return true;
        }
        if modifiers.control && modifiers.shift && code == b'C' as u16 {
            let text = self.terminal.lock().unwrap().screen().contents();
            copy_text(hwnd, &text);
            return true;
        }
        if modifiers.shift && matches!(code, VK_PRIOR | VK_NEXT) {
            let lines = i32::from(self.size.0).saturating_sub(1);
            self.terminal
                .lock()
                .unwrap()
                .scroll(if code == VK_PRIOR { lines } else { -lines });
            return true;
        }
        let key = match code {
            VK_UP => Key::Up,
            VK_DOWN => Key::Down,
            VK_LEFT => Key::Left,
            VK_RIGHT => Key::Right,
            VK_HOME => Key::Home,
            VK_END => Key::End,
            VK_INSERT => Key::Insert,
            VK_DELETE => Key::Delete,
            VK_PRIOR => Key::PageUp,
            VK_NEXT => Key::PageDown,
            VK_TAB => Key::Tab,
            VK_F1..=VK_F12 => Key::Function((code - VK_F1 + 1) as u8),
            _ => return false,
        };
        let application = self.terminal.lock().unwrap().screen().application_cursor();
        self.send(input::key(key, modifiers, application));
        true
    }

    fn tick(&mut self, hwnd: HWND) -> bool {
        if let Some(session) = &self.session {
            while let Some(event) = session.try_event() {
                match event {
                    Event::Resized(_, _) => {}
                    Event::Exited(status) => {
                        self.status = status;
                        return true;
                    }
                    Event::Error(error) => {
                        self.error = Some(error);
                        self.status = 1;
                        return true;
                    }
                }
            }
        }
        let mut terminal = self.terminal.lock().unwrap();
        let mut redraw = terminal.take_dirty();
        let title = terminal.take_title();
        drop(terminal);
        if self.cursor_tick.elapsed() >= Duration::from_millis(500) {
            self.cursor_tick = Instant::now();
            self.cursor_visible = !self.cursor_visible;
            redraw = true;
        }
        // SAFETY: the caller dispatches a timer for this live window. Neither
        // call reenters a state-borrowing message handler.
        unsafe {
            if let Some(title) = title {
                let title = wide(&if title.is_empty() || title == "winsh" {
                    "winsh".to_owned()
                } else {
                    format!("{title} — winsh")
                });
                SetWindowTextW(hwnd, title.as_ptr());
            }
            if redraw {
                InvalidateRect(hwnd, ptr::null(), 0);
            }
        }
        false
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
            let previous_font = SelectObject(memory, self.fonts.handles[0]);
            let background = CreateSolidBrush(DEFAULT_BACKGROUND);
            FillRect(memory, &client, background);
            DeleteObject(background);

            let terminal = self.terminal.lock().unwrap();
            let screen = terminal.screen();
            let (rows, columns) = screen.size();
            for row in 0..rows {
                for column in 0..columns {
                    let Some(cell) = screen.cell(row, column) else {
                        continue;
                    };
                    if cell.is_wide_continuation() {
                        continue;
                    }
                    let x = PADDING + i32::from(column) * self.fonts.width;
                    let y = PADDING + i32::from(row) * self.fonts.height;
                    let width = self.fonts.width * if cell.is_wide() { 2 } else { 1 };
                    let rect = RECT {
                        left: x,
                        top: y,
                        right: x + width,
                        bottom: y + self.fonts.height,
                    };
                    let mut foreground = color(cell.fgcolor(), DEFAULT_FOREGROUND);
                    let mut background = color(cell.bgcolor(), DEFAULT_BACKGROUND);
                    if cell.inverse() {
                        std::mem::swap(&mut foreground, &mut background);
                    }
                    SetTextColor(memory, foreground);
                    SetBkColor(memory, background);
                    let index = usize::from(cell.bold())
                        + 2 * usize::from(cell.italic())
                        + 4 * usize::from(cell.underline());
                    SelectObject(memory, self.fonts.handles[index]);
                    let text: Vec<u16> = if cell.has_contents() {
                        OsStr::new(cell.contents()).encode_wide().collect()
                    } else {
                        vec![32]
                    };
                    ExtTextOutW(
                        memory,
                        x,
                        y,
                        ETO_OPAQUE | ETO_CLIPPED,
                        &rect,
                        text.as_ptr(),
                        text.len() as u32,
                        ptr::null(),
                    );
                }
            }
            if self.focused
                && self.cursor_visible
                && !screen.hide_cursor()
                && screen.scrollback() == 0
            {
                let (row, column) = screen.cursor_position();
                if row < rows && column < columns {
                    let x = PADDING + i32::from(column) * self.fonts.width;
                    let y = PADDING + i32::from(row + 1) * self.fonts.height;
                    let rect = RECT {
                        left: x,
                        top: y - 2,
                        right: x + self.fonts.width,
                        bottom: y,
                    };
                    let brush = CreateSolidBrush(DEFAULT_FOREGROUND);
                    FillRect(memory, &rect, brush);
                    DeleteObject(brush);
                }
            }
            drop(terminal);
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
                if let Ok(fonts) = Fonts::new((wparam as u32 & 0xffff).max(96)) {
                    (*state).fonts = fonts;
                }
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
                InvalidateRect(hwnd, ptr::null(), 0);
                0
            }
            WM_PAINT if !state.is_null() => {
                (*state).paint(hwnd);
                0
            }
            WM_TIMER if !state.is_null() => {
                let close = (*state).tick(hwnd);
                if close {
                    DestroyWindow(hwnd);
                }
                0
            }
            WM_KEYDOWN | WM_SYSKEYDOWN if !state.is_null() => {
                let handled = (*state).key_down(wparam as u16, modifiers(), hwnd);
                if handled {
                    0
                } else {
                    DefWindowProcW(hwnd, message, wparam, lparam)
                }
            }
            WM_CHAR | WM_SYSCHAR if !state.is_null() => {
                let modifiers = modifiers();
                let unit = wparam as u16;
                // Tab is handled in WM_KEYDOWN, and clipboard shortcuts must
                // not also inject the translated control character.
                if unit == 9 || (modifiers.control && modifiers.shift && matches!(unit, 3 | 22)) {
                    return 0;
                }
                let mut text = (*state).text.push(if unit == 8 { 127 } else { unit });
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
                (*state).wheel += (wparam >> 16) as i16 as i32;
                let lines = (*state).wheel / 120 * 3;
                (*state).wheel %= 120;
                (*state).terminal.lock().unwrap().scroll(lines);
                0
            }
            _ => DefWindowProcW(hwnd, message, wparam, lparam),
        }
    }
}

struct Fonts {
    handles: Vec<HFONT>,
    width: i32,
    height: i32,
}

impl Fonts {
    fn new(dpi: u32) -> io::Result<Self> {
        let mut fonts = Self {
            handles: Vec::new(),
            width: 1,
            height: 1,
        };
        // SAFETY: valid font parameters and static face name. Each created font
        // is owned by Fonts and deleted only after being deselected from a DC.
        unsafe {
            for index in 0..8 {
                let font = CreateFontW(
                    -(((12 * dpi + 36) / 72) as i32),
                    0,
                    0,
                    0,
                    if index & 1 != 0 {
                        FW_BOLD as i32
                    } else {
                        FW_NORMAL as i32
                    },
                    u32::from(index & 2 != 0),
                    u32::from(index & 4 != 0),
                    0,
                    DEFAULT_CHARSET as u32,
                    OUT_DEFAULT_PRECIS as u32,
                    CLIP_DEFAULT_PRECIS as u32,
                    CLEARTYPE_QUALITY as u32,
                    (FIXED_PITCH | FF_MODERN) as u32,
                    w!("Cascadia Mono"),
                );
                if font.is_null() {
                    return Err(io::Error::last_os_error());
                }
                fonts.handles.push(font);
            }
            let dc = GetDC(ptr::null_mut());
            if dc.is_null() {
                return Err(io::Error::last_os_error());
            }
            let old = SelectObject(dc, fonts.handles[0]);
            let mut metrics = TEXTMETRICW::default();
            let success = GetTextMetricsW(dc, &mut metrics);
            SelectObject(dc, old);
            ReleaseDC(ptr::null_mut(), dc);
            if success == 0 {
                return Err(io::Error::last_os_error());
            }
            fonts.width = metrics.tmAveCharWidth.max(1);
            fonts.height = (metrics.tmHeight + metrics.tmExternalLeading).max(1);
        }
        Ok(fonts)
    }
}

impl Drop for Fonts {
    fn drop(&mut self) {
        for handle in &self.handles {
            // SAFETY: uniquely owned fonts, deselected after every paint.
            unsafe { DeleteObject(*handle) };
        }
    }
}

fn modifiers() -> Modifiers {
    // SAFETY: querying the thread's keyboard state needs no pointers.
    unsafe {
        Modifiers {
            shift: GetKeyState(VK_SHIFT as i32) < 0,
            alt: GetKeyState(VK_MENU as i32) < 0,
            control: GetKeyState(VK_CONTROL as i32) < 0,
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
