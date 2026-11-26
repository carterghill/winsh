use super::settings::FontSettings;
use std::{cell::RefCell, collections::HashMap, io, ptr};
use windows_sys::{
    Win32::{Foundation::HANDLE, Graphics::Gdi::*},
    core::w,
};

/// Private to this process. State drops all HFONTs before the registration.
pub(super) struct Symbols {
    font: ttf_parser::Face<'static>,
    resource: HANDLE,
}

impl Symbols {
    pub(super) fn load() -> io::Result<Self> {
        static DATA: &[u8] = include_bytes!("../../assets/SymbolsNerdFontMono-Regular.ttf");
        let font = ttf_parser::Face::parse(DATA, 0).map_err(io::Error::other)?;
        let mut count = 0;
        // SAFETY: static font data and a writable registration count.
        let resource = unsafe {
            AddFontMemResourceEx(
                DATA.as_ptr().cast(),
                DATA.len() as u32,
                ptr::null(),
                &raw mut count,
            )
        };
        if resource.is_null() {
            return Err(io::Error::other("could not load the bundled symbols font"));
        }
        Ok(Self { font, resource })
    }

    pub(super) fn glyph(&self, contents: &str) -> Option<u16> {
        let mut chars = contents.chars();
        let ch = chars.next()?;
        if matches!(ch as u32, 0xe000..=0xf8ff | 0xf0000..=0xffffd | 0x100000..=0x10fffd)
            && chars.next().is_none()
        {
            self.font.glyph_index(ch).map(|glyph| glyph.0)
        } else {
            None
        }
    }
}

impl Drop for Symbols {
    fn drop(&mut self) {
        // SAFETY: this is the registration returned by AddFontMemResourceEx.
        unsafe { RemoveFontMemResourceEx(self.resource) };
    }
}

pub(super) struct Fonts {
    text: Vec<Font>,
    label: Font,
    icon: Font,
    compact_symbol: SymbolSize,
    spaced_symbol: SymbolSize,
    pub(super) width: i32,
    pub(super) height: i32,
    pub(super) cursor_y: std::ops::Range<i32>,
}

impl Fonts {
    pub(super) fn new(dpi: u32, settings: &FontSettings) -> io::Result<Self> {
        let em = (settings.font_size_px * dpi as f32 / 96.0).round().max(1.0) as i32;
        let family = super::wide(&settings.font_family);
        let mut text = Vec::with_capacity(8);
        for style in 0..8 {
            text.push(create_font(em, style, family.as_ptr())?);
        }
        let metrics = font_metrics(text[0].0)?;
        let width = metrics.tmAveCharWidth.max(1);
        let height = (metrics.tmHeight + metrics.tmExternalLeading).max(1);
        // Sample both cap height and x-height. Weighting the two letters by their
        // ink puts the icon alongside the visible body of a mixed-case filename.
        let text_center = with_font_dc(text[0].0, |dc| {
            let mut weight = 0.0;
            let mut moment = 0.0;
            for ch in ['H', 'x'] {
                if let Some(ink) = glyph_ink(dc, u32::from(ch), 0) {
                    weight += ink.weight;
                    moment += ink.center * ink.weight;
                }
            }
            Ok(if weight > 0.0 {
                metrics.tmAscent as f32 + moment / weight
            } else {
                height as f32 / 2.0
            })
        })?;
        // Use the font's em height, extending through the descenders.
        let bottom = metrics.tmHeight.clamp(1, height);
        // The bundled symbols have a one-em advance. A blank neighbor allows
        // the larger face to draw beyond a single text cell.
        Ok(Self {
            text,
            label: create_font((13 * dpi as i32 + 48) / 96, 0, w!("Segoe UI"))?,
            icon: create_font((12 * dpi as i32 + 48) / 96, 0, w!("Segoe MDL2 Assets"))?,
            compact_symbol: SymbolSize::new(em.min(width), height, text_center)?,
            spaced_symbol: SymbolSize::new(em.min(width + width / 2), height, text_center)?,
            width,
            height,
            cursor_y: metrics.tmInternalLeading.clamp(0, bottom - 1)..bottom,
        })
    }

    pub(super) fn text(&self, style: usize) -> HFONT {
        self.text[style].0
    }

    pub(super) fn label(&self) -> HFONT {
        self.label.0
    }

    pub(super) fn icon(&self) -> HFONT {
        self.icon.0
    }

    pub(super) fn symbol(&self, spaced: bool) -> &SymbolSize {
        if spaced {
            &self.spaced_symbol
        } else {
            &self.compact_symbol
        }
    }
}

pub(super) struct SymbolSize {
    font: Font,
    pub(super) advance: i32,
    ascent: i32,
    cell_height: i32,
    text_center: f32,
    fallback_y: i32,
    offsets: RefCell<HashMap<u16, i32>>,
}

impl SymbolSize {
    fn new(em: i32, cell_height: i32, text_center: f32) -> io::Result<Self> {
        let font = create_font(em, 0, w!("Symbols Nerd Font Mono"))?;
        let metrics = font_metrics(font.0)?;
        Ok(Self {
            font,
            advance: metrics.tmAveCharWidth,
            ascent: metrics.tmAscent,
            cell_height,
            text_center,
            fallback_y: (cell_height - metrics.tmHeight) / 2,
            offsets: RefCell::default(),
        })
    }

    pub(super) fn handle(&self) -> HFONT {
        self.font.0
    }

    pub(super) fn y(&self, glyph: u16) -> i32 {
        *self.offsets.borrow_mut().entry(glyph).or_insert_with(|| {
            let ink = with_font_dc(self.font.0, |dc| {
                Ok(glyph_ink(dc, u32::from(glyph), GGO_GLYPH_INDEX))
            });
            let Ok(Some(ink)) = ink else {
                return self.fallback_y;
            };
            let baseline = (self.text_center - ink.center).round() as i32;
            let top = (baseline + ink.top).clamp(0, (self.cell_height - ink.height).max(0));
            top - ink.top - self.ascent
        })
    }
}

struct Ink {
    // Vertical coordinates are relative to the font baseline, increasing downwards.
    top: i32,
    height: i32,
    center: f32,
    weight: f32,
}

/// Weight each scanline by the actual coverage of its strokes. The folder tab,
/// open outline and monitor stand contribute differently to the visual center.
fn glyph_ink(dc: HDC, glyph: u32, flags: u32) -> Option<Ink> {
    let mut matrix = MAT2::default();
    matrix.eM11.value = 1;
    matrix.eM22.value = 1;
    let mut bounds = GLYPHMETRICS::default();
    // SAFETY: query the required byte count, then supply a buffer of that size.
    let coverage = unsafe {
        let format = GGO_GRAY8_BITMAP | flags;
        let len = GetGlyphOutlineW(dc, glyph, format, &mut bounds, 0, ptr::null_mut(), &matrix);
        if len == 0 || len == u32::MAX {
            return None;
        }
        let mut data = vec![0u8; len as usize];
        if GetGlyphOutlineW(
            dc,
            glyph,
            format,
            &mut bounds,
            len,
            data.as_mut_ptr().cast(),
            &matrix,
        ) == u32::MAX
        {
            return None;
        }
        data
    };
    let width = bounds.gmBlackBoxX as usize;
    if width == 0 {
        return None;
    }
    let mut total = 0.0;
    let mut moment = 0.0;
    for (y, row) in coverage.chunks_exact(width.next_multiple_of(4)).enumerate() {
        let weight = row[..width].iter().map(|&v| u32::from(v)).sum::<u32>() as f32;
        total += weight;
        moment += (y as f32 + 0.5) * weight;
    }
    let top = -bounds.gmptGlyphOrigin.y;
    (total > 0.0).then(|| Ink {
        top,
        height: bounds.gmBlackBoxY as i32,
        center: top as f32 + moment / total,
        weight: total,
    })
}

struct Font(HFONT);

impl Drop for Font {
    fn drop(&mut self) {
        // SAFETY: the creating thread owns this handle and restores each DC after drawing.
        unsafe { DeleteObject(self.0) };
    }
}

fn create_font(em: i32, style: usize, face: *const u16) -> io::Result<Font> {
    // SAFETY: the caller supplies a NUL-terminated face name valid for this call.
    let font = unsafe {
        CreateFontW(
            -em,
            0, // Let GDI preserve the face's natural width at this em size.
            0,
            0,
            if style & 1 == 0 { FW_NORMAL } else { FW_BOLD } as i32,
            u32::from(style & 2 != 0),
            u32::from(style & 4 != 0),
            0,
            DEFAULT_CHARSET as u32,
            OUT_DEFAULT_PRECIS as u32,
            CLIP_DEFAULT_PRECIS as u32,
            CLEARTYPE_QUALITY as u32,
            (FIXED_PITCH | FF_MODERN) as u32,
            face,
        )
    };
    if font.is_null() {
        Err(io::Error::last_os_error())
    } else {
        Ok(Font(font))
    }
}

fn font_metrics(font: HFONT) -> io::Result<TEXTMETRICW> {
    with_font_dc(font, |dc| {
        let mut metrics = TEXTMETRICW::default();
        // SAFETY: the screen DC holds the requested font and metrics is writable.
        if unsafe { GetTextMetricsW(dc, &mut metrics) } == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(metrics)
        }
    })
}

fn with_font_dc<T>(font: HFONT, measure: impl FnOnce(HDC) -> io::Result<T>) -> io::Result<T> {
    // SAFETY: borrow a screen DC, restore its selected object, then release the DC.
    unsafe {
        let dc = GetDC(ptr::null_mut());
        if dc.is_null() {
            return Err(io::Error::last_os_error());
        }
        let previous = SelectObject(dc, font);
        let result = measure(dc);
        SelectObject(dc, previous);
        ReleaseDC(ptr::null_mut(), dc);
        result
    }
}

#[cfg(test)]
mod font_tests {
    use super::*;

    #[test]
    fn icons_keep_the_font_proportions_at_both_render_sizes() {
        let symbols = Symbols::load().unwrap();
        assert_eq!(symbols.glyph("D"), None);
        for dpi in [96, 120, 144, 192] {
            let fonts = Fonts::new(dpi, &FontSettings::default()).unwrap();
            for spaced in [false, true] {
                check_glyphs(&symbols, &fonts, spaced);
            }
        }
    }

    fn check_glyphs(symbols: &Symbols, fonts: &Fonts, spaced: bool) {
        let size = fonts.symbol(spaced);
        let font = size.handle();
        // SAFETY: use a memory DC and restore its font before releasing either object.
        unsafe {
            let dc = CreateCompatibleDC(ptr::null_mut());
            assert!(!dc.is_null());
            let previous = SelectObject(dc, font);
            let mut name = [0; 64];
            let count = GetTextFaceW(dc, name.len() as i32, name.as_mut_ptr());
            assert!(count > 0);
            assert_eq!(
                String::from_utf16_lossy(&name[..count as usize - 1]),
                "Symbols Nerd Font Mono"
            );
            let metrics = font_metrics(font).unwrap();
            let pixels_per_unit = (metrics.tmHeight - metrics.tmInternalLeading) as f32
                / f32::from(symbols.font.units_per_em());
            let mut matrix = MAT2::default();
            matrix.eM11.value = 1;
            matrix.eM22.value = 1;
            for sample in ["\u{f108}", "\u{f024d}", "\u{f024f}", "\u{f115}", "\u{f18d}"] {
                let glyph = symbols.glyph(sample).unwrap();
                let design = symbols
                    .font
                    .glyph_bounding_box(ttf_parser::GlyphId(glyph))
                    .unwrap();
                let mut bounds = GLYPHMETRICS::default();
                let bytes = GetGlyphOutlineW(
                    dc,
                    u32::from(glyph),
                    GGO_GRAY8_BITMAP | GGO_GLYPH_INDEX,
                    &mut bounds,
                    0,
                    ptr::null_mut(),
                    &matrix,
                );
                assert!(bytes > 0 && bytes != u32::MAX, "no bitmap for {sample}");
                let y = size.y(glyph);
                let ink = glyph_ink(dc, u32::from(glyph), GGO_GLYPH_INDEX).unwrap();
                let painted_center = (y + metrics.tmAscent) as f32 + ink.center;
                assert!(
                    (painted_center - size.text_center).abs() <= 0.51,
                    "{sample} is vertically off-center"
                );
                let top = y + metrics.tmAscent - bounds.gmptGlyphOrigin.y;
                let right = bounds.gmptGlyphOrigin.x + bounds.gmBlackBoxX as i32;
                let available = if spaced { 2 * fonts.width } else { fonts.width };
                assert!(top >= 0 && top + bounds.gmBlackBoxY as i32 <= fonts.height);
                assert!(bounds.gmptGlyphOrigin.x >= 0 && right <= available);
                if spaced && sample == "\u{f024d}" {
                    assert!(
                        right > fonts.width && right < available,
                        "folder should fill part of the spacer"
                    );
                }
                // Pixel snapping may round either edge. Both axes must use the same scale.
                assert!(
                    (bounds.gmBlackBoxX as f32 - f32::from(design.width()) * pixels_per_unit).abs()
                        <= 2.0,
                    "horizontal distortion in {sample}"
                );
                assert!(
                    (bounds.gmBlackBoxY as f32 - f32::from(design.height()) * pixels_per_unit)
                        .abs()
                        <= 2.0,
                    "vertical distortion in {sample}"
                );
            }
            SelectObject(dc, previous);
            DeleteDC(dc);
        }
    }
}
