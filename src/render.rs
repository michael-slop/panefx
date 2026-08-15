//! GDI cell renderer: turn the shared fire grid into glyphs on a panel.
//!
//! Drawing goes to an off-screen bitmap and is blitted in one `BitBlt`.
//! Painting glyphs straight to the window DC at 30fps would tear and flicker
//! badly — the panel is a full-window repaint every frame.

use windows::core::PCWSTR;
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Gdi::{
    BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, CreateFontW, CreateSolidBrush,
    DeleteDC, DeleteObject, FillRect, GetDC, ReleaseDC, SelectObject, SetBkMode, SetTextColor,
    TextOutW, CLEARTYPE_QUALITY, DEFAULT_CHARSET, DEFAULT_PITCH, FF_DONTCARE, FW_NORMAL, HBITMAP, HBRUSH, HDC, HFONT, HGDIOBJ,
    OUT_TT_PRECIS, SRCCOPY, TRANSPARENT,
};

use crate::animation::AsciiAnimation;

/// Fallback font face if config supplies none.
///
/// BigBlueTerm is a DOS/CP437 face with NO katakana — which is exactly why the
/// rain effect uses CP437 glyphs instead of the usual Matrix katakana.
pub const FALLBACK_FONT: &str = "BigBlueTerm437 Nerd Font Mono";

/// Ask GDI which face it ACTUALLY selected for `requested`, and warn if it
/// substituted something else.
///
/// This exists because a substitution is completely silent: `CreateFontW`
/// succeeds, drawing succeeds, and you get the wrong typeface with no error
/// anywhere. Checking that the family resolves via .NET/GDI+ does not catch it
/// either — only `GetTextFaceW` on a DC with the font selected tells the truth.
/// (Real case: `ANSI_CHARSET` silently gave Arial for BigBlueTerm for this
/// program's entire early history.)
pub fn verify_font(requested: &str) -> Option<String> {
    use windows::Win32::Graphics::Gdi::GetTextFaceW;
    unsafe {
        let dc = CreateCompatibleDC(HDC::default());
        if dc.is_invalid() {
            return None;
        }
        let face: Vec<u16> = requested.encode_utf16().chain(std::iter::once(0)).collect();
        let font = CreateFontW(
            16,
            0,
            0,
            0,
            FW_NORMAL.0 as i32,
            0,
            0,
            0,
            DEFAULT_CHARSET.0.into(),
            OUT_TT_PRECIS.0.into(),
            0,
            CLEARTYPE_QUALITY.0.into(),
            (DEFAULT_PITCH.0 | FF_DONTCARE.0).into(),
            PCWSTR(face.as_ptr()),
        );
        let old = SelectObject(dc, font);
        let mut buf = [0u16; 64];
        let n = GetTextFaceW(dc, Some(&mut buf));
        SelectObject(dc, old);
        let _ = DeleteObject(font);
        let _ = DeleteDC(dc);
        if n <= 0 {
            return None;
        }
        let got = String::from_utf16_lossy(&buf[..(n as usize).saturating_sub(1)]);
        Some(got)
    }
}



/// Repaint in response to `WM_PAINT` when we have no fresh frame to draw.
/// The animation loop drives real frames; this just keeps the window from
/// showing garbage if Windows asks for a repaint between frames.
pub fn paint_cached(_hdc: HDC, _hwnd: HWND) {
    // Intentionally empty: `draw_fire` paints the whole window every frame,
    // so a stale-region repaint has nothing useful to add.
}

/// Draw an animation into `hwnd`, clipped to `width` x `height` pixels.
///
/// Works against the `AsciiAnimation` trait, not a concrete effect, so adding
/// a new effect needs no change here.
///
/// The panel blits its own sub-rect of the shared simulation, so N panels cost
/// N blits but only ONE simulation step per frame.
pub fn draw_animation(
    panel: &mut crate::panel::Panel,
    anim: &dyn AsciiAnimation,
    cfg: &crate::config::Config,
) {
    let hwnd = panel.hwnd;
    let (width, height) = (panel.width, panel.height);
    let (cell_w, cell_h) = (cfg.cell_w, cfg.cell_h);
    let face_name = if cfg.font.trim().is_empty() {
        FALLBACK_FONT
    } else {
        cfg.font.as_str()
    };
    if width <= 0 || height <= 0 || cell_w <= 0 || cell_h <= 0 {
        return;
    }

    unsafe {
        let hdc = GetDC(hwnd);
        if hdc.is_invalid() {
            return;
        }

        // --- ensure the cache exists and matches the current inputs ---
        //
        // Everything below used to be built from scratch every frame, for every
        // panel. `CreateFontW` alone runs the font mapper; the bitmap is a
        // full-window DIB allocation. Now each is rebuilt only when the thing it
        // depends on actually changes.
        if panel.gdi.is_none() {
            let mem_dc = CreateCompatibleDC(hdc);
            if mem_dc.is_invalid() {
                ReleaseDC(hwnd, hdc);
                return;
            }
            SetBkMode(mem_dc, TRANSPARENT);
            panel.gdi = Some(crate::panel::GdiCache {
                mem_dc,
                bitmap: HBITMAP::default(),
                bmp_w: 0,
                bmp_h: 0,
                old_bmp: HGDIOBJ::default(),
                font: HFONT::default(),
                font_face: String::new(),
                font_h: 0,
                old_font: HGDIOBJ::default(),
                brush: HBRUSH::default(),
                brush_colour: u32::MAX,
                run: Vec::with_capacity(256),
                face_utf16: Vec::new(),
            });
        }
        let g = panel.gdi.as_mut().unwrap();
        let mem_dc = g.mem_dc;

        // Bitmap: rebuild only on resize.
        if g.bitmap.is_invalid() || g.bmp_w != width || g.bmp_h != height {
            let new_bmp = CreateCompatibleBitmap(hdc, width, height);
            let prev = SelectObject(mem_dc, new_bmp);
            // Keep the DC's ORIGINAL bitmap (from the first swap only), so it
            // can be restored at Drop. Later swaps return our own old bitmap,
            // which we delete instead.
            if g.old_bmp.is_invalid() {
                g.old_bmp = prev;
            } else if !g.bitmap.is_invalid() {
                let _ = DeleteObject(HGDIOBJ(g.bitmap.0));
            }
            g.bitmap = new_bmp;
            g.bmp_w = width;
            g.bmp_h = height;
        }

        // Font: rebuild only when the face or the cell height changes.
        if g.font.is_invalid() || g.font_face != face_name || g.font_h != cell_h {
            // NOTE: the UTF-16 buffer MUST outlive the CreateFontW call. Writing
            // `PCWSTR(name.encode_utf16().collect::<Vec<_>>().as_ptr())` inline
            // creates a temporary that is dropped at the end of the expression,
            // leaving CreateFontW reading freed memory.
            g.face_utf16 = face_name
                .encode_utf16()
                .chain(std::iter::once(0))
                .collect();
            let new_font = CreateFontW(
                cell_h,
                0,
                0,
                0,
                FW_NORMAL.0 as i32,
                0,
                0,
                0,
                // DEFAULT_CHARSET, never ANSI_CHARSET.
                //
                // ANSI_CHARSET makes GDI *silently substitute Arial* for
                // BigBlueTerm437 Nerd Font Mono — verified with GetTextFaceW on
                // the real DC. Nothing errors; you just get Arial. Consolas
                // happens to survive ANSI_CHARSET, which is exactly why this
                // went unnoticed for so long: the font looked fine until the
                // face was changed.
                DEFAULT_CHARSET.0.into(),
                OUT_TT_PRECIS.0.into(),
                0,
                CLEARTYPE_QUALITY.0.into(),
                (DEFAULT_PITCH.0 | FF_DONTCARE.0).into(),
                PCWSTR(g.face_utf16.as_ptr()),
            );
            let prev = SelectObject(mem_dc, new_font);
            if g.old_font.is_invalid() {
                g.old_font = prev;
            } else if !g.font.is_invalid() {
                let _ = DeleteObject(HGDIOBJ(g.font.0));
            }
            g.font = new_font;
            g.font_face = face_name.to_string();
            g.font_h = cell_h;
        }

        // Brush: rebuild only when the effect's background colour changes.
        let bg_colour = anim.background().colorref();
        if g.brush.is_invalid() || g.brush_colour != bg_colour {
            if !g.brush.is_invalid() {
                let _ = DeleteObject(HGDIOBJ(g.brush.0));
            }
            g.brush = CreateSolidBrush(windows::Win32::Foundation::COLORREF(bg_colour));
            g.brush_colour = bg_colour;
        }

        let full = RECT {
            left: 0,
            top: 0,
            right: width,
            bottom: height,
        };
        FillRect(mem_dc, &full, g.brush);

        // Inset to match Alacritty's own text padding, so the animation's
        // bottom row lands on the terminal's bottom text row rather than
        // behind the padding strip below it.
        let (pad_x, pad_y) = (cfg.pad_x, cfg.pad_y);
        // Crop eats into the TOP of the drawable area only — the bottom stays
        // anchored so the flames remain rooted at the last text row.
        let crop = cfg.crop_top;
        let usable_w = (width - pad_x * 2).max(0);
        let usable_h = (height - pad_y * 2 - crop).max(0);

        let (anim_cols, anim_rows) = anim.dimensions();
        let cols = (usable_w / cell_w).min(anim_cols as i32);
        let rows = (usable_h / cell_h).min(anim_rows as i32);

        // Bottom-align the grid: any pixels left over after the integer cell
        // division belong at the TOP (as cold sky), never at the bottom, or
        // the flames float above the terminal's last text row.
        let y_offset = pad_y + crop + (usable_h - rows * cell_h).max(0);
        let x_offset = pad_x;

        // Single pass over the grid, emitting each cell once.
        //
        // The obvious structure — an outer loop over the 8 ramp levels, each
        // rescanning every cell — costs `levels * cols * rows` iterations per
        // panel per frame. Measured on pHub that pegged a full core (15.3 CPU
        // seconds in 16s wall with two 143x85 panels). One pass, tracking the
        // last colour set, is ~7x fewer iterations for the same output.
        //
        // Runs of identical glyph+colour are batched into a single TextOutW,
        // which matters because these effects produce long horizontal runs.
        let mut current_colour: Option<u32> = None;
        // Reused scratch buffer — this used to be a fresh allocation per panel
        // per frame.
        let mut run = std::mem::take(&mut g.run);
        run.clear();

        for row in 0..rows {
            let mut run_start_col = 0i32;
            // (glyph, packed colour) of the run currently being accumulated.
            let mut run_key: Option<(char, u32)> = None;

            // `cols + 1` so the final run is always flushed by the sentinel.
            for col in 0..=cols {
                let key = if col == cols {
                    None // sentinel: force a flush
                } else {
                    anim.cell_at(col as usize, row as usize)
                        .map(|(g, c)| (g, c.colorref()))
                };

                if key == run_key {
                    if let Some((g, _)) = key {
                        let mut buf = [0u16; 2];
                        run.extend_from_slice(g.encode_utf16(&mut buf));
                    }
                    continue;
                }

                // Flush the finished run. `None` means the animation asked for
                // nothing to be drawn, so there is nothing to flush.
                if let Some((_, colour)) = run_key {
                    if !run.is_empty() {
                        if current_colour != Some(colour) {
                            SetTextColor(mem_dc, windows::Win32::Foundation::COLORREF(colour));
                            current_colour = Some(colour);
                        }
                        let _ = TextOutW(
                            mem_dc,
                            x_offset + run_start_col * cell_w,
                            y_offset + row * cell_h,
                            &run,
                        );
                    }
                }

                run.clear();
                run_key = key;
                run_start_col = col;
                if let Some((g, _)) = key {
                    let mut buf = [0u16; 2];
                    run.extend_from_slice(g.encode_utf16(&mut buf));
                }
            }
            run.clear();
        }

        // --- present ---
        let _ = BitBlt(hdc, 0, 0, width, height, mem_dc, 0, 0, SRCCOPY);

        // Hand the scratch buffer back for next frame. The DC, bitmap, font and
        // brush all STAY selected and alive — they are freed in `Panel::drop`.
        g.run = run;
        ReleaseDC(hwnd, hdc);
    }
}
