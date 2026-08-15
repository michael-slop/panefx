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
    TextOutW, CLEARTYPE_QUALITY, DEFAULT_CHARSET, DEFAULT_PITCH, FF_DONTCARE, FW_NORMAL, HDC,
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
    hwnd: HWND,
    anim: &dyn AsciiAnimation,
    cfg: &crate::config::Config,
    width: i32,
    height: i32,
) {
    let (cell_w, cell_h) = (cfg.cell_w, cfg.cell_h);
    let font = if cfg.font.trim().is_empty() { FALLBACK_FONT } else { cfg.font.as_str() };
    if width <= 0 || height <= 0 || cell_w <= 0 || cell_h <= 0 {
        return;
    }

    unsafe {
        let hdc = GetDC(hwnd);
        if hdc.is_invalid() {
            return;
        }

        // --- off-screen buffer ---
        let mem_dc = CreateCompatibleDC(hdc);
        let bitmap = CreateCompatibleBitmap(hdc, width, height);
        let old_bmp = SelectObject(mem_dc, bitmap);

        // Background comes from the animation, not a global constant.
        let bg = CreateSolidBrush(windows::Win32::Foundation::COLORREF(
            anim.background().colorref(),
        ));
        let full = RECT {
            left: 0,
            top: 0,
            right: width,
            bottom: height,
        };
        FillRect(mem_dc, &full, bg);
        let _ = DeleteObject(bg);

        // Monospace font sized to the cell. Matches Alacritty's configured
        // face so the fire lines up with the terminal grid.
        //
        // NOTE: `face` MUST be bound to a local. Writing
        // `PCWSTR(name.encode_utf16().collect::<Vec<_>>().as_ptr())` inline
        // creates a temporary Vec that is dropped at the end of the
        // expression, leaving CreateFontW reading freed memory.
        let face: Vec<u16> = font.encode_utf16().chain(std::iter::once(0)).collect();
        let font = CreateFontW(
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
            // BigBlueTerm437 Nerd Font Mono — verified with GetTextFaceW on the
            // real DC. Nothing errors; you just get Arial. Consolas happens to
            // survive ANSI_CHARSET, which is exactly why this went unnoticed
            // for so long: the font looked fine until the face was changed.
            //
            // Nerd Fonts carry huge glyph coverage and report a charset that
            // ANSI_CHARSET refuses to match. Checking the family resolves in
            // .NET/GDI+ does NOT catch this — only GetTextFaceW on the DC after
            // SelectObject tells the truth.
            DEFAULT_CHARSET.0.into(),
            OUT_TT_PRECIS.0.into(),
            0,
            CLEARTYPE_QUALITY.0.into(),
            (DEFAULT_PITCH.0 | FF_DONTCARE.0).into(),
            PCWSTR(face.as_ptr()),
        );
        let old_font = SelectObject(mem_dc, font);
        SetBkMode(mem_dc, TRANSPARENT);

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
        let mut run: Vec<u16> = Vec::with_capacity(cols.max(1) as usize);

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

        SelectObject(mem_dc, old_font);
        let _ = DeleteObject(font);
        SelectObject(mem_dc, old_bmp);
        let _ = DeleteObject(bitmap);
        let _ = DeleteDC(mem_dc);
        ReleaseDC(hwnd, hdc);
    }
}
