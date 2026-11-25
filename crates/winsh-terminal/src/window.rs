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
mod titlebar;
use fonts::{Fonts, Symbols};
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
    let mut state = Box::new(State::new(program, arguments, dpi)?);
    // SAFETY: static system cursor, borrowed for the window class lifetime.
    let cursor = unsafe { LoadCursorW(ptr::null_mut(), IDC_IBEAM) };
    let class = WNDCLASSW {
        style: CS_HREDRAW | CS_VREDRAW | CS_DBLCLKS,
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
            // A tab shortcut belongs entirely to the window. Translating it
            // would queue a control character for the newly selected shell.
            if !(message.message == WM_KEYDOWN
                && tab_shortcut(message.wParam as u16, modifiers()).is_some())
            {
                TranslateMessage(&message);
            }
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
    default_title: String,
    text: TextInput,
    wheel: i32,
}

impl Tab {
    fn spawn(
        program: &Path,
        arguments: &[OsString],
        (rows, columns): (u16, u16),
        number: usize,
    ) -> io::Result<Self> {
        let terminal = Arc::new(Mutex::new(Terminal::new(rows, columns)));
        let session = Session::spawn(program, arguments, rows, columns, Arc::clone(&terminal))?;
        let shell = program
            .file_stem()
            .unwrap_or(program.as_os_str())
            .to_string_lossy();
        let default_title = if number == 1 {
            shell.into_owned()
        } else {
            format!("{shell} {number}")
        };
        Ok(Self {
            terminal,
            session,
            title: default_title.clone(),
            default_title,
            text: TextInput::default(),
            wheel: 0,
        })
    }
}

struct State {
    tabs: Vec<Tab>,
    active: usize,
    next_number: usize,
    program: PathBuf,
    arguments: Vec<OsString>,
    layout: Layout,
    dpi: u32,
    hover: Option<Control>,
    pressed: Option<Control>,
    title_dirty: bool,
    fonts: Fonts,
    symbols: Symbols,
    focused: bool,
    cursor_visible: bool,
    cursor_tick: Instant,
    size: (u16, u16),
    error: Option<String>,
    status: i32,
}

impl State {
    fn new(program: &Path, arguments: &[OsString], dpi: u32) -> io::Result<Self> {
        let symbols = Symbols::load()?;
        let size = (30, 100);
        Ok(Self {
            tabs: vec![Tab::spawn(program, arguments, size, 1)?],
            active: 0,
            next_number: 2,
            program: program.to_owned(),
            arguments: arguments.to_owned(),
            layout: Layout::new(dpi, 0, 1, 0, 0),
            dpi,
            hover: None,
            pressed: None,
            title_dirty: true,
            fonts: Fonts::new(dpi)?,
            symbols,
            focused: false,
            cursor_visible: true,
            cursor_tick: Instant::now(),
            size,
            error: None,
            status: 0,
        })
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
                let tab = Tab::spawn(&self.program, &self.arguments, self.size, self.next_number)?;
                self.next_number += 1;
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

    fn key_down(&mut self, code: u16, modifiers: Modifiers, hwnd: HWND) -> bool {
        if (modifiers.control && modifiers.shift && code == b'V' as u16)
            || (modifiers.shift && code == VK_INSERT)
        {
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
            return true;
        }
        if modifiers.control && modifiers.shift && code == b'C' as u16 {
            let text = self.current().terminal.lock().unwrap().screen().contents();
            copy_text(hwnd, &text);
            return true;
        }
        if modifiers.shift && matches!(code, VK_PRIOR | VK_NEXT) {
            let lines = i32::from(self.size.0).saturating_sub(1);
            self.current()
                .terminal
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
        let application = self
            .current()
            .terminal
            .lock()
            .unwrap()
            .screen()
            .application_cursor();
        self.send(input::key(key, modifiers, application));
        true
    }

    fn tick(&mut self) -> bool {
        let mut redraw = false;
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
                let title =
                    if title.is_empty() || self.program.file_stem() == Some(OsStr::new(&title)) {
                        tab.default_title.clone()
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
                    let contents = cell.contents();
                    let symbol = self.symbols.glyph(contents);
                    let mut wide = cell.is_wide();
                    if symbol.is_some()
                        && !wide
                        && can_share_symbol_space(screen.cell(row, column + 1), background)
                    {
                        // Paint the blank's background together with the icon so it
                        // cannot erase the right edge on the next iteration.
                        cells.next();
                        wide = true;
                    }
                    let x = padding + i32::from(column) * self.fonts.width;
                    let y = origin_y + i32::from(row) * self.fonts.height;
                    let width = self.fonts.width * if wide { 2 } else { 1 };
                    let rect = RECT {
                        left: x,
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
                        let size = self.fonts.symbol(wide);
                        // In a padded icon field, align its advance to the end of
                        // the field. The following space is the visible name gap.
                        let dx = if wide
                            && can_share_symbol_space(
                                screen.cell(row, column.saturating_add(2)),
                                background,
                            ) {
                            (width - size.advance).max(0)
                        } else {
                            0
                        };
                        (
                            size.handle(),
                            std::slice::from_ref(glyph),
                            ETO_GLYPH_INDEX,
                            dx,
                            size.y(*glyph),
                        )
                    } else {
                        units = if contents.is_empty() {
                            vec![32]
                        } else {
                            OsStr::new(contents).encode_wide().collect()
                        };
                        (self.fonts.text(index), units.as_slice(), 0, 0, 0)
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

enum Shortcut {
    New,
    Close,
    Cycle { backward: bool },
}

fn tab_shortcut(code: u16, modifiers: Modifiers) -> Option<Shortcut> {
    if !modifiers.control || modifiers.alt {
        return None;
    }
    match code {
        VK_TAB => Some(Shortcut::Cycle {
            backward: modifiers.shift,
        }),
        0x54 if modifiers.shift => Some(Shortcut::New),
        0x57 if modifiers.shift => Some(Shortcut::Close),
        _ => None,
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
                if let Ok(fonts) = Fonts::new(dpi) {
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
            WM_SYSKEYDOWN if wparam == usize::from(VK_F4) => {
                DefWindowProcW(hwnd, message, wparam, lparam)
            }
            WM_KEYDOWN | WM_SYSKEYDOWN if !state.is_null() => {
                let mods = modifiers();
                if let Some(shortcut) = tab_shortcut(wparam as u16, mods) {
                    if lparam & (1 << 30) == 0 {
                        let index = (*state).active;
                        let count = (*state).tabs.len();
                        let control = match shortcut {
                            Shortcut::New => Control::New,
                            Shortcut::Close => Control::CloseTab(index),
                            Shortcut::Cycle { backward } => Control::Tab(if backward {
                                (index + count - 1) % count
                            } else {
                                (index + 1) % count
                            }),
                        };
                        activate(hwnd, state, control);
                    }
                    return 0;
                }
                let handled = (*state).key_down(wparam as u16, mods, hwnd);
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
                }
                0
            }
            WM_LBUTTONUP if !state.is_null() => {
                let pt = point(lparam);
                let pressed = (*state).pressed.take();
                let hit = (*state).layout.hit(pt.x, pt.y);
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
                invalidate_bar(hwnd, &(*state).layout);
                0
            }
            _ => DefWindowProcW(hwnd, message, wparam, lparam),
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
    fn background_tab_can_update_and_exit_while_the_selected_shell_keeps_its_state() {
        let program = PathBuf::from(std::env::var_os("ComSpec").unwrap());
        let mut state = State::new(&program, &["/d".into(), "/q".into()], 96).unwrap();
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
