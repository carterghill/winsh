use super::{DEFAULT_BACKGROUND, DEFAULT_FOREGROUND, rgb};
use windows_sys::Win32::{
    Foundation::{POINT, RECT},
    Graphics::Gdi::*,
};

const BAR: u32 = rgb(31, 35, 43);
const EDGE: u32 = rgb(52, 58, 69);
const HOVER: u32 = rgb(47, 53, 64);
const MUTED: u32 = rgb(168, 176, 190);
const ACCENT: u32 = rgb(125, 211, 196);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Control {
    Tab(usize),
    CloseTab(usize),
    New,
    Previous,
    Next,
    Minimize,
    Maximize,
    CloseWindow,
}

pub(super) fn px(dpi: u32, value: i32) -> i32 {
    (value * dpi as i32 + 48) / 96
}

pub(super) struct Layout {
    dpi: u32,
    pub width: i32,
    pub height: i32,
    pub first: usize,
    pub end: usize,
    items: Vec<(Control, RECT)>,
}

impl Layout {
    pub fn new(dpi: u32, width: i32, count: usize, active: usize, first: usize) -> Self {
        let s = |n| px(dpi, n);
        let height = s(42);
        let caption_width = s(46);
        let caption_left = width - 3 * caption_width;
        let mut x = s(8);
        // Always leave a drag target between the tab tools and window controls.
        let available = (caption_left - x - s(36) - s(34)).max(0);
        let overflow = count > (available / s(112)).max(1) as usize;
        let available = (available - if overflow { 2 * s(26) } else { 0 }).max(0);
        let visible = count.min((available / s(112)).max(1) as usize);
        let first = first
            .min(count - visible)
            .min(active)
            .max((active + 1).saturating_sub(visible));
        let end = first + visible;
        let mut items = Vec::new();
        let tab_width = (available / visible.max(1) as i32).min(s(190));
        for index in first..end {
            items.push((Control::Tab(index), rect(x, s(6), x + tab_width, height)));
            items.push((
                Control::CloseTab(index),
                rect(
                    x + tab_width - s(32),
                    s(11),
                    x + tab_width - s(6),
                    height - s(5),
                ),
            ));
            x += tab_width;
        }
        x += s(3);
        if overflow {
            for control in [Control::Previous, Control::Next] {
                items.push((control, rect(x, s(8), x + s(26), height - s(4))));
                x += s(26);
            }
        }
        items.push((Control::New, rect(x, s(8), x + s(31), height - s(4))));
        for (i, control) in [Control::Minimize, Control::Maximize, Control::CloseWindow]
            .into_iter()
            .enumerate()
        {
            let left = caption_left + i as i32 * caption_width;
            items.push((control, rect(left, 0, left + caption_width, height - 1)));
        }
        Self {
            dpi,
            width,
            height,
            first,
            end,
            items,
        }
    }

    pub fn hit(&self, x: i32, y: i32) -> Option<Control> {
        self.items
            .iter()
            .rev()
            .find(|(_, r)| x >= r.left && x < r.right && y >= r.top && y < r.bottom)
            .map(|(control, _)| *control)
    }

    pub fn paint(&self, dc: HDC, state: &super::State, maximized: bool) {
        let s = |n| px(self.dpi, n);
        // SAFETY: the caller owns this memory DC. SaveDC restores every GDI selection.
        unsafe {
            let saved = SaveDC(dc);
            let brush = GetStockObject(DC_BRUSH);
            SelectObject(dc, brush);
            SelectObject(dc, GetStockObject(NULL_PEN));
            SetBkMode(dc, TRANSPARENT as i32);
            let fill = |r: RECT, color| {
                SetDCBrushColor(dc, color);
                FillRect(dc, &r, brush);
            };
            let rounded = |r: RECT, radius, color| {
                SetDCBrushColor(dc, color);
                RoundRect(dc, r.left, r.top, r.right + 1, r.bottom + 1, radius, radius);
            };
            fill(rect(0, 0, self.width, self.height), BAR);
            fill(rect(0, self.height - 1, self.width, self.height), EDGE);
            for &(control, r) in &self.items {
                let hovered = state.hover == Some(control);
                let disabled = match control {
                    Control::Previous => self.first == 0,
                    Control::Next => self.end == state.tabs.len(),
                    _ => false,
                };
                let color = if disabled {
                    rgb(96, 105, 120)
                } else if hovered {
                    DEFAULT_FOREGROUND
                } else {
                    MUTED
                };
                let glyph = match control {
                    Control::Tab(index) => {
                        let active = index == state.active;
                        let hot = hovered || state.hover == Some(Control::CloseTab(index));
                        if active || hot {
                            let surface = if active { DEFAULT_BACKGROUND } else { HOVER };
                            rounded(r, s(12), if active { EDGE } else { surface });
                            let inside = rect(r.left + 1, r.top + 1, r.right - 1, r.bottom);
                            rounded(inside, s(10), surface);
                            fill(
                                rect(inside.left, r.bottom - s(8), inside.right, r.bottom),
                                surface,
                            );
                        } else if index + 1 != state.active {
                            fill(
                                rect(r.right - 1, r.top + s(9), r.right, r.bottom - s(8)),
                                EDGE,
                            );
                        }
                        let x = r.left + s(10);
                        let y = (r.top + r.bottom - s(18)) / 2;
                        rounded(
                            rect(x, y, x + s(20), y + s(18)),
                            s(8),
                            if active { rgb(32, 58, 57) } else { EDGE },
                        );
                        let pen = CreatePen(PS_SOLID, s(1), if active { ACCENT } else { MUTED });
                        let previous = SelectObject(dc, pen);
                        let caret = [
                            POINT {
                                x: x + s(5),
                                y: y + s(5),
                            },
                            POINT {
                                x: x + s(9),
                                y: y + s(9),
                            },
                            POINT {
                                x: x + s(5),
                                y: y + s(13),
                            },
                        ];
                        Polyline(dc, caret.as_ptr(), caret.len() as i32);
                        MoveToEx(dc, x + s(10), y + s(13), std::ptr::null_mut());
                        LineTo(dc, x + s(15), y + s(13));
                        SelectObject(dc, previous);
                        DeleteObject(pen);
                        let label = rect(r.left + s(36), r.top, r.right - s(33), r.bottom);
                        draw_text(
                            dc,
                            state.fonts.label(),
                            &state.tabs[index].title,
                            label,
                            if active { DEFAULT_FOREGROUND } else { MUTED },
                            DT_END_ELLIPSIS,
                        );
                        continue;
                    }
                    Control::CloseTab(index) => {
                        if index != state.active
                            && !hovered
                            && state.hover != Some(Control::Tab(index))
                        {
                            continue;
                        }
                        "\u{e711}"
                    }
                    Control::New => "\u{e710}",
                    Control::Previous => "\u{e76b}",
                    Control::Next => "\u{e76c}",
                    Control::Minimize => "\u{e921}",
                    Control::Maximize if maximized => "\u{e923}",
                    Control::Maximize => "\u{e922}",
                    Control::CloseWindow => "\u{e8bb}",
                };
                if hovered && !disabled {
                    match control {
                        Control::CloseWindow => fill(r, rgb(180, 49, 62)),
                        Control::Minimize | Control::Maximize => fill(r, HOVER),
                        _ => rounded(
                            r,
                            s(9),
                            if state.pressed == Some(control) {
                                EDGE
                            } else {
                                HOVER
                            },
                        ),
                    }
                }
                draw_text(dc, state.fonts.icon(), glyph, r, color, DT_CENTER);
            }
            RestoreDC(dc, saved);
        }
    }
}

fn rect(left: i32, top: i32, right: i32, bottom: i32) -> RECT {
    RECT {
        left,
        top,
        right,
        bottom,
    }
}

fn draw_text(dc: HDC, font: HFONT, text: &str, mut r: RECT, color: u32, flags: DRAW_TEXT_FORMAT) {
    let units: Vec<_> = text.encode_utf16().collect();
    // SAFETY: the DC and font are live, and DrawTextW only reads the specified units.
    unsafe {
        SelectObject(dc, font);
        SetTextColor(dc, color);
        DrawTextW(
            dc,
            units.as_ptr(),
            units.len() as i32,
            &mut r,
            DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX | flags,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crowded_title_bar_keeps_the_active_tab_and_window_controls_reachable() {
        for dpi in [96, 120, 144, 192] {
            for width in [404, 800, 1100] {
                let mut first = 0;
                for active in (0..20).chain((0..20).rev()) {
                    let layout = Layout::new(dpi, px(dpi, width), 20, active, first);
                    first = layout.first;
                    assert!((first..layout.end).contains(&active));
                    for &(control, r) in &layout.items {
                        assert!(r.left >= 0 && r.right <= layout.width && r.left < r.right);
                        assert_eq!(
                            layout.hit((r.left + r.right) / 2, (r.top + r.bottom) / 2),
                            Some(control)
                        );
                    }
                    let y = layout.height / 2;
                    let new = layout
                        .items
                        .iter()
                        .find(|(c, _)| *c == Control::New)
                        .unwrap()
                        .1;
                    assert_eq!(layout.hit(new.right + px(dpi, 10), y), None);
                    assert_eq!(layout.hit(new.left, layout.height), None);
                }
            }
        }
    }
}
