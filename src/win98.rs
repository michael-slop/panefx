//! The Windows 98 look, ported to egui.
//!
//! Every value here is **lifted from `slopkit/internal/win98/`**, which in turn
//! lifted them from michaelslop.org (`site/static/style.css` and `98.css`). They
//! are not re-derived by eye: the whole point is that panefx, slopkit and
//! s0nar.slop are visibly the same family, and three independent guesses at
//! "Windows 98 grey" would not be.
//!
//! # What actually makes it read as Win98
//!
//! Two things, and neither is the colour:
//!
//! 1. **The double bevel ring.** Not a border — four tones in two nested 1px
//!    rings, light on the top-left and dark on the bottom-right (or inverted,
//!    for a sunken well). That asymmetry is what says "3D chrome" rather than
//!    "a box with an outline".
//! 2. **Zero rounding, anywhere.** One rounded corner and the illusion is gone.
//!
//! # The font
//!
//! Embedded, never requested by name. slopkit's note is the reason, and it
//! applies harder here: BigBlueTerm437 is installed on exactly one computer, and
//! asking for it by name collapses the design to "a grey window" on any other.
//! panefx's own renderer *does* request it by name — that is fine, it draws on
//! Michael's desk only. A GUI that might be run anywhere carries its own copy.
//!
//! Licence: **CC BY-SA 4.0**, via Nerd Fonts, from VileR's Ultimate Oldschool PC
//! Font Pack. Redistribution and embedding are permitted; the one obligation
//! reaching this code is **attribution**, which is why [`ATTRIBUTION`] exists and
//! the About box shows it.

use eframe::egui::{self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, Rect, Stroke, Vec2};

/// The attribution the About box must show. See the module header — this is a
/// licence obligation, not a courtesy.
pub const ATTRIBUTION: &str = "\
BigBlueTerm437 Nerd Font Mono — CC BY-SA 4.0
Patched by Nerd Fonts from VileR's Ultimate Oldschool PC Font Pack
(int10h.org/oldschool-pc-fonts), a reproduction of the IBM PC CP437 face.";

/// Every tone the theme uses.
///
/// One struct with two consts rather than a trait: the palettes differ only in
/// their values, and a runtime swap is a field assignment.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Palette {
    pub desktop: Color32,
    pub button_face: Color32,

    /// The bevel ring, outer then inner, top-left then bottom-right.
    pub bevel_white: Color32,
    pub bevel_light: Color32,
    pub bevel_shadow: Color32,
    pub bevel_dark: Color32,

    pub title_start: Color32,
    pub title_end: Color32,
    pub title_inactive: Color32,
    pub title_inact_end: Color32,

    pub text: Color32,
    pub muted: Color32,

    /// Accent and selection ground, and the text that sits on it.
    pub navy: Color32,
    pub white: Color32,

    pub yellow: Color32,
    pub field_bg: Color32,
    pub panel_bg: Color32,
    pub bone: Color32,

    pub ok: Color32,
    pub bad: Color32,
    pub warn: Color32,
    pub black: Color32,
}

const fn rgb(hex: u32) -> Color32 {
    Color32::from_rgb(
        ((hex >> 16) & 0xFF) as u8,
        ((hex >> 8) & 0xFF) as u8,
        (hex & 0xFF) as u8,
    )
}

impl Palette {
    /// michaelslop.org exactly as it renders in a browser.
    pub const LIGHT: Palette = Palette {
        desktop: rgb(0x008080), // --desktop teal
        button_face: rgb(0xC0C0C0), // --btn-face silver

        bevel_white: rgb(0xFFFFFF),  // outer top/left
        bevel_light: rgb(0xDFDFDF),  // inner top/left
        bevel_shadow: rgb(0x808080), // inner bottom/right
        bevel_dark: rgb(0x0A0A0A),   // outer bottom/right

        title_start: rgb(0x000080), // --navy
        title_end: rgb(0x1084D0),
        title_inactive: rgb(0x808080),
        title_inact_end: rgb(0xB5B5B5),

        text: rgb(0x222222),  // --fg
        muted: rgb(0x4A4A4A), // --muted

        navy: rgb(0x000080),
        white: rgb(0xFFFFFF),

        yellow: rgb(0xFFCC00),
        field_bg: rgb(0xFFFFFF),
        panel_bg: rgb(0xD4D0C8),
        bone: rgb(0xECE9DD),

        ok: rgb(0x007A00),
        bad: rgb(0xA80000),
        warn: rgb(0x8A6D00),
        black: rgb(0x000000),
    };

    /// The same scheme taken down, derived rather than invented — see slopkit's
    /// `palette.go`, which records why each tone moved where it did.
    pub const DARK: Palette = Palette {
        desktop: rgb(0x06201F),     // teal, taken well down
        button_face: rgb(0x2B2B2B), // the silver equivalent

        bevel_white: rgb(0x6E6E6E),  // was #fff, the lightest edge
        bevel_light: rgb(0x4A4A4A),  // was #dfdfdf
        bevel_shadow: rgb(0x1A1A1A), // was #808080
        bevel_dark: rgb(0x000000),   // was #0a0a0a, true black

        title_start: rgb(0x3A3A3A),
        title_end: rgb(0x5E5E5E),
        title_inactive: rgb(0x242424),
        title_inact_end: rgb(0x363636),

        text: rgb(0xD8D8D8),
        muted: rgb(0x9A9A9A),

        navy: rgb(0x4F4F4F),
        white: rgb(0xEDEDED),

        yellow: rgb(0xFFCC00), // the one hue that stays: it is the wordmark
        field_bg: rgb(0x1E1E1E),
        panel_bg: rgb(0x242424),
        bone: rgb(0xECE9DD), // the skull stays bone

        ok: rgb(0x4CAF50),
        bad: rgb(0xE06C60),
        warn: rgb(0xD8A93C),
        black: rgb(0x000000),
    };
}

/// Which way the 3D edge faces.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Bevel {
    /// A button or panel standing proud: light on the top-left.
    Raised,
    /// A well you look into — a text field, a preview pane. The same four tones,
    /// inverted, so it reads as a hole rather than a bump.
    Sunken,
    /// Single-depth sunken, for status-bar fields where a full ring is too
    /// heavy. The inner ring repeats the outer and is invisible.
    Thin,
}

/// Which bevel a toggling button wears, given whether it is ON.
///
/// Win98 buttons show state as DEPTH: raised when idle, pushed in when
/// selected. Trivial, and defined here anyway because it was got backwards --
/// the tab strip drew the selected tab raised while the monitor buttons drew it
/// sunken, so the two halves of the same window disagreed about which way "on"
/// looked. One function, one answer, and a test that pins it.
pub fn toggle_bevel(on: bool) -> Bevel {
    if on {
        Bevel::Sunken
    } else {
        Bevel::Raised
    }
}

/// The face colour matching `toggle_bevel`: a pushed-in button shows the
/// recessed field colour, an idle one the standard button face.
pub fn toggle_face(on: bool, p: &Palette) -> Color32 {
    if on {
        p.field_bg
    } else {
        p.button_face
    }
}

/// The double ring: 1px outer + 1px inner on each side.
pub const BEVEL_THICKNESS: f32 = 2.0;

impl Bevel {
    /// `(outer_tl, outer_br, inner_tl, inner_br)` for this style.
    pub fn tones(self, p: &Palette) -> (Color32, Color32, Color32, Color32) {
        match self {
            Bevel::Sunken => (p.bevel_shadow, p.bevel_white, p.bevel_dark, p.bevel_light),
            Bevel::Thin => (p.bevel_shadow, p.bevel_white, p.bevel_shadow, p.bevel_white),
            Bevel::Raised => (p.bevel_white, p.bevel_dark, p.bevel_light, p.bevel_shadow),
        }
    }
}

/// The eight rectangles of a bevel ring, in draw order, for a rect of `size`.
///
/// Returned as data rather than drawn directly so the geometry is testable
/// without a window — it is the single thing that makes this read as Win98, and
/// "it looked right on my screen" is not a check.
///
/// 98.css builds this from four stacked inset box-shadows:
///
/// ```text
/// inset -1px -1px #0a0a0a   outer bottom/right, near black
/// inset  1px  1px #ffffff   outer top/left, white
/// inset -2px -2px #808080   inner bottom/right, mid grey
/// inset  2px  2px #dfdfdf   inner top/left, light grey
/// ```
///
/// egui has no inset shadow either, so the rings are eight rectangles. More
/// objects than a border, and the only way to get the asymmetric two-tone edge.
pub fn bevel_rects(rect: Rect, style: Bevel, p: &Palette) -> [(Rect, Color32); 8] {
    let (otl, obr, itl, ibr) = style.tones(p);
    let (x, y, w, h) = (rect.min.x, rect.min.y, rect.width(), rect.height());
    let r = |dx: f32, dy: f32, rw: f32, rh: f32| {
        Rect::from_min_size(egui::pos2(x + dx, y + dy), Vec2::new(rw.max(0.0), rh.max(0.0)))
    };
    [
        (r(0.0, 0.0, w, 1.0), otl),           // outer top
        (r(0.0, 0.0, 1.0, h), otl),           // outer left
        (r(0.0, h - 1.0, w, 1.0), obr),       // outer bottom
        (r(w - 1.0, 0.0, 1.0, h), obr),       // outer right
        (r(1.0, 1.0, w - 2.0, 1.0), itl),     // inner top
        (r(1.0, 1.0, 1.0, h - 2.0), itl),     // inner left
        (r(1.0, h - 2.0, w - 2.0, 1.0), ibr), // inner bottom
        (r(w - 2.0, 1.0, 1.0, h - 2.0), ibr), // inner right
    ]
}

/// Paint a bevel ring, and optionally fill inside it.
pub fn bevel(painter: &egui::Painter, rect: Rect, style: Bevel, p: &Palette, fill: Option<Color32>) {
    if let Some(c) = fill {
        painter.rect_filled(rect, CornerRadius::ZERO, c);
    }
    for (r, c) in bevel_rects(rect, style, p) {
        painter.rect_filled(r, CornerRadius::ZERO, c);
    }
}

/// Title-bar height. From 98.css: 18px of text plus 3px padding top and bottom.
pub const TITLE_BAR_HEIGHT: f32 = 22.0;
/// Caption text size, and its left inset.
pub const TITLE_TEXT_SIZE: f32 = 13.0;
const TITLE_TEXT_X: f32 = 4.0;

/// Paint the gradient title bar with its caption.
///
/// The gradient is horizontal, left to right — vertical is a later Windows.
/// egui has no gradient primitive, so it is drawn as vertical 1px strips; at
/// this height that is a few hundred rects and costs nothing.
pub fn title_bar(painter: &egui::Painter, rect: Rect, caption: &str, active: bool, p: &Palette) {
    let (a, b) = if active {
        (p.title_start, p.title_end)
    } else {
        (p.title_inactive, p.title_inact_end)
    };
    let w = rect.width().max(1.0);
    let steps = w.ceil() as usize;
    for i in 0..steps {
        let t = i as f32 / (steps.max(2) - 1) as f32;
        let c = lerp_colour(a, b, t);
        let strip = Rect::from_min_size(
            egui::pos2(rect.min.x + i as f32, rect.min.y),
            Vec2::new(1.0, rect.height()),
        );
        painter.rect_filled(strip, CornerRadius::ZERO, c);
    }
    painter.text(
        egui::pos2(rect.min.x + TITLE_TEXT_X, rect.center().y),
        egui::Align2::LEFT_CENTER,
        caption,
        egui::FontId::new(TITLE_TEXT_SIZE, FontFamily::Monospace),
        p.white,
    );
}

/// Linear blend between two colours.
pub fn lerp_colour(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let m = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t) as u8;
    Color32::from_rgb(m(a.r(), b.r()), m(a.g(), b.g()), m(a.b(), b.b()))
}

/// The embedded pixel font. See the module header for why it is embedded.
const FONT: &[u8] = include_bytes!("../assets/BigBlueTerm437NerdFontMono-Regular.ttf");

/// Register the font as BOTH families.
///
/// Proportional as well as monospace on purpose: michaelslop.org uses this face
/// for body text, not just code, and leaving egui's default sans for labels
/// would give a window with two typefaces fighting each other.
pub fn install_font(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    fonts.font_data.insert(
        "bigblue".to_owned(),
        std::sync::Arc::new(FontData::from_static(FONT)),
    );
    for fam in [FontFamily::Proportional, FontFamily::Monospace] {
        fonts.families.entry(fam).or_default().insert(0, "bigblue".to_owned());
    }
    ctx.set_fonts(fonts);
}

/// A Windows 98 fader: a sunken trough with a raised thumb.
///
/// **Created, not ported.** s0nar.slop's DESIGN.md describes "a beveled trough
/// with a raised thumb", but slopkit never built one -- `voicepanel.go` uses a
/// stock Fyne slider. So this is the idiom being written down for the first
/// time, from the `Bevel` primitive.
///
/// Returns `Some(new_value)` only when the value actually CHANGED, so a caller
/// can send a command per change rather than per frame. A slider that fires
/// every frame while held would flood the daemon with identical writes.
pub fn fader(
    ui: &mut egui::Ui,
    p: &Palette,
    value: i64,
    min: i64,
    max: i64,
    width: f32,
) -> Option<i64> {
    let height = 22.0;
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, height), egui::Sense::click_and_drag());
    let painter = ui.painter();

    // The trough is a thin sunken groove down the middle, not the full height:
    // a full-height well would leave no room for the thumb to stand proud of.
    let groove_h = 6.0;
    let groove = Rect::from_min_size(
        egui::pos2(rect.min.x, rect.center().y - groove_h / 2.0),
        Vec2::new(rect.width(), groove_h),
    );
    bevel(painter, groove, Bevel::Sunken, p, Some(p.field_bg));

    let span = (max - min).max(1) as f32;
    let t = ((value - min) as f32 / span).clamp(0.0, 1.0);

    // The thumb is inset by its own width so it never hangs off either end.
    let thumb_w = 11.0;
    let travel = (rect.width() - thumb_w).max(1.0);
    let thumb = Rect::from_min_size(
        egui::pos2(rect.min.x + t * travel, rect.min.y),
        Vec2::new(thumb_w, height),
    );
    bevel(painter, thumb, Bevel::Raised, p, Some(p.button_face));

    // Clicking anywhere on the trough jumps there -- Win98 pages by a step, but
    // jump-to-click is what a mouse user expects now and it is strictly less
    // fiddly on a 300px slider with a 5000-wide range.
    if resp.dragged() || resp.clicked() {
        if let Some(pos) = resp.interact_pointer_pos() {
            let x = (pos.x - rect.min.x - thumb_w / 2.0).clamp(0.0, travel);
            let nt = x / travel;
            let nv = min + (nt * span).round() as i64;
            let nv = nv.clamp(min, max);
            if nv != value {
                return Some(nv);
            }
        }
    }
    None
}

/// A colour swatch: a sunken well showing the colour, clickable.
///
/// Sunken rather than raised because it is a sample you look INTO, the way a
/// field is -- a raised swatch reads as a button that happens to be coloured.
pub fn swatch(ui: &mut egui::Ui, p: &Palette, colour: Color32, size: Vec2) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click());
    bevel(ui.painter(), rect, Bevel::Sunken, p, Some(colour));
    resp
}

/// A hard-edged segment meter. Blocks, never a gradient or a smooth bar.
///
/// `value` is 0..1. Segments light up left to right; the unlit ones stay as
/// visible wells so the meter's full range is always readable, which is what
/// stops a quiet meter looking like a broken one.
pub fn segment_meter(
    ui: &mut egui::Ui,
    p: &Palette,
    value: f32,
    segments: usize,
    size: Vec2,
) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::hover());
    bevel(ui.painter(), rect, Bevel::Sunken, p, Some(p.field_bg));
    let n = segments.max(1);
    let inner = rect.shrink(BEVEL_THICKNESS + 1.0);
    let gap = 1.0;
    let seg_w = ((inner.width() - gap * (n - 1) as f32) / n as f32).max(1.0);
    let lit = (value.clamp(0.0, 1.0) * n as f32).round() as usize;
    for i in 0..n {
        let x = inner.min.x + i as f32 * (seg_w + gap);
        let r = Rect::from_min_size(egui::pos2(x, inner.min.y), Vec2::new(seg_w, inner.height()));
        // Green through amber to red: the meter says "how close to the top" as
        // well as "how much", which a single colour cannot.
        let frac = i as f32 / n as f32;
        let c = if i < lit {
            if frac > 0.85 {
                p.bad
            } else if frac > 0.65 {
                p.warn
            } else {
                p.ok
            }
        } else {
            p.panel_bg
        };
        ui.painter().rect_filled(r, CornerRadius::ZERO, c);
    }
    resp
}

/// Theme sizes, from slopkit's `theme.go`. The numbers are the design.
pub mod size {
    pub const TEXT: f32 = 14.0;
    pub const INLINE_ICON: f32 = 16.0;
    pub const PADDING: f32 = 4.0;
    pub const INNER_PADDING: f32 = 6.0;
    pub const INPUT_BORDER: f32 = 1.0;
    pub const SCROLLBAR: f32 = 16.0; // Win98 scrollbars are chunky
    pub const SEPARATOR: f32 = 1.0;
}

/// Apply the palette and the measured sizes to egui.
///
/// **Every rounding is zeroed.** One rounded corner and the illusion is gone,
/// which is why this is done wholesale rather than per-widget.
pub fn apply_theme(ctx: &egui::Context, p: &Palette) {
    install_font(ctx);

    // egui 0.36 keeps a style PER THEME, and picks between them by the
    // system/user preference. panefx has exactly one look at a time, so the
    // same style is written to both slots -- otherwise the window silently
    // reverts to egui's defaults when Windows is set to the other mode.
    let theme = if *p == Palette::DARK { egui::Theme::Dark } else { egui::Theme::Light };
    let mut style = (*ctx.style_of(theme)).clone();
    let v = &mut style.visuals;

    v.dark_mode = *p == Palette::DARK;
    v.panel_fill = p.button_face;
    v.window_fill = p.button_face;
    v.extreme_bg_color = p.field_bg;
    v.faint_bg_color = p.panel_bg;
    v.override_text_color = Some(p.text);

    // Win98 INVERTS to navy on hover rather than tinting, so hover and focus
    // are the same accent ground rather than two shades of the base.
    for w in [
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
        &mut v.widgets.open,
        &mut v.widgets.noninteractive,
    ] {
        w.corner_radius = CornerRadius::ZERO;
        w.bg_fill = p.button_face;
        w.weak_bg_fill = p.button_face;
        w.bg_stroke = Stroke::new(size::INPUT_BORDER, p.bevel_shadow);
        w.fg_stroke = Stroke::new(1.0, p.text);
        w.expansion = 0.0;
    }
    // A Win98 button does NOT invert on hover -- it stays face-coloured and its
    // bevel flips on PRESS. Only menu items and list selections invert to navy.
    // Painting hover navy made every focused button look permanently selected,
    // which is what the chrome proof showed.
    v.widgets.hovered.bg_fill = p.button_face;
    v.widgets.hovered.weak_bg_fill = p.button_face;
    v.widgets.hovered.fg_stroke = Stroke::new(1.0, p.text);
    // Pressed: the ring inverts, so the face reads as pushed in.
    v.widgets.active.bg_fill = p.button_face;
    v.widgets.active.weak_bg_fill = p.button_face;
    v.widgets.active.fg_stroke = Stroke::new(1.0, p.text);
    v.widgets.active.bg_stroke = Stroke::new(size::INPUT_BORDER, p.bevel_dark);
    // `open` is a dropdown showing its menu -- that one IS a selection.
    v.widgets.open.bg_fill = p.navy;
    v.widgets.open.weak_bg_fill = p.navy;
    v.widgets.open.fg_stroke = Stroke::new(1.0, p.white);

    v.selection.bg_fill = p.navy;
    v.selection.stroke = Stroke::new(1.0, p.white);

    v.window_corner_radius = CornerRadius::ZERO;
    v.menu_corner_radius = CornerRadius::ZERO;
    // Hard shadows only, never soft.
    v.window_shadow = egui::epaint::Shadow::NONE;
    v.popup_shadow = egui::epaint::Shadow::NONE;

    let s = &mut style.spacing;
    s.item_spacing = Vec2::splat(size::PADDING);
    s.button_padding = Vec2::new(size::INNER_PADDING, size::PADDING);
    s.scroll.bar_width = size::SCROLLBAR;

    style.text_styles.insert(
        egui::TextStyle::Body,
        egui::FontId::new(size::TEXT, FontFamily::Monospace),
    );
    style.text_styles.insert(
        egui::TextStyle::Button,
        egui::FontId::new(size::TEXT, FontFamily::Monospace),
    );
    style.text_styles.insert(
        egui::TextStyle::Monospace,
        egui::FontId::new(size::TEXT, FontFamily::Monospace),
    );

    let style = std::sync::Arc::new(style);
    ctx.set_style_of(egui::Theme::Light, style.clone());
    ctx.set_style_of(egui::Theme::Dark, style);
    // And pin the preference, so the window does not follow the OS into a
    // theme the palette was not built for.
    ctx.set_theme(theme);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(w: f32, h: f32) -> Rect {
        Rect::from_min_size(egui::pos2(10.0, 20.0), Vec2::new(w, h))
    }

    #[test]
    fn the_ring_is_eight_rects_with_no_gap_between_them() {
        // THE thing that makes this read as Win98. The outer ring is flush at
        // the edge and the inner ring is inset by exactly 1px -- a gap, or a
        // 2px outer, and it stops looking like chrome and starts looking like
        // a box with an outline.
        let rects = bevel_rects(r(40.0, 30.0), Bevel::Raised, &Palette::LIGHT);
        assert_eq!(rects.len(), 8);
        // outer top sits at the very edge and spans the full width
        assert_eq!(rects[0].0.min, egui::pos2(10.0, 20.0));
        assert_eq!(rects[0].0.width(), 40.0);
        assert_eq!(rects[0].0.height(), 1.0);
        // inner top is inset by one pixel on every side
        assert_eq!(rects[4].0.min, egui::pos2(11.0, 21.0));
        assert_eq!(rects[4].0.width(), 38.0);
        // outer bottom/right hug the far edge
        assert_eq!(rects[2].0.min.y, 20.0 + 30.0 - 1.0);
        assert_eq!(rects[3].0.min.x, 10.0 + 40.0 - 1.0);
    }

    #[test]
    fn raised_is_light_on_the_top_left_and_sunken_is_inverted() {
        // The asymmetry IS the 3D. Symmetric tones read as a flat outline, and
        // getting the two styles the same way round is the classic slip.
        let p = &Palette::LIGHT;
        let raised = bevel_rects(r(20.0, 20.0), Bevel::Raised, p);
        let sunken = bevel_rects(r(20.0, 20.0), Bevel::Sunken, p);
        // top-left of a raised surface is the lightest tone
        assert_eq!(raised[0].1, p.bevel_white);
        assert_eq!(raised[2].1, p.bevel_dark);
        // and a sunken one is the exact opposite
        assert_eq!(sunken[0].1, p.bevel_shadow);
        assert_eq!(sunken[2].1, p.bevel_white);
        assert_ne!(raised[0].1, sunken[0].1);
    }

    #[test]
    fn thin_repeats_its_outer_ring() {
        // Single depth: the inner ring is drawn but invisible, which keeps the
        // renderer uniform rather than needing a four-rect special case.
        let p = &Palette::LIGHT;
        let t = bevel_rects(r(20.0, 20.0), Bevel::Thin, p);
        assert_eq!(t[0].1, t[4].1, "inner top must repeat outer top");
        assert_eq!(t[2].1, t[6].1, "inner bottom must repeat outer bottom");
    }

    #[test]
    fn a_degenerate_rect_produces_no_negative_sizes() {
        // Sizes come from a layout pass, so 0x0 and 1x1 have to be survivable:
        // a negative width panics egui's painter rather than drawing nothing.
        for (w, h) in [(0.0, 0.0), (1.0, 1.0), (2.0, 2.0), (3.0, 1.0)] {
            for (rect, _) in bevel_rects(r(w, h), Bevel::Raised, &Palette::LIGHT) {
                assert!(rect.width() >= 0.0, "{w}x{h} gave a negative width");
                assert!(rect.height() >= 0.0, "{w}x{h} gave a negative height");
            }
        }
    }

    #[test]
    fn the_light_palette_is_michaelslop_org() {
        // Pinned against the values in slopkit's palette.go. These are lifted,
        // not chosen -- panefx, slopkit and s0nar.slop must be the same family,
        // and three independent guesses at "Windows 98 grey" would not be.
        let p = Palette::LIGHT;
        assert_eq!(p.desktop, Color32::from_rgb(0x00, 0x80, 0x80));
        assert_eq!(p.button_face, Color32::from_rgb(0xC0, 0xC0, 0xC0));
        assert_eq!(p.bevel_white, Color32::WHITE);
        assert_eq!(p.bevel_dark, Color32::from_rgb(0x0A, 0x0A, 0x0A));
        assert_eq!(p.title_start, Color32::from_rgb(0x00, 0x00, 0x80));
        assert_eq!(p.title_end, Color32::from_rgb(0x10, 0x84, 0xD0));
    }

    #[test]
    fn the_dark_palette_keeps_the_two_hues_that_carry_identity() {
        // Everything else desaturates; these two do not, and slopkit records
        // why: the yellow IS the wordmark, and the skull stays bone.
        assert_eq!(Palette::DARK.yellow, Palette::LIGHT.yellow);
        assert_eq!(Palette::DARK.bone, Palette::LIGHT.bone);
        // And the bevel ring must still run light-to-dark, or the 3D inverts.
        let d = Palette::DARK;
        assert!(d.bevel_white.r() > d.bevel_light.r());
        assert!(d.bevel_light.r() > d.bevel_shadow.r());
        assert!(d.bevel_shadow.r() > d.bevel_dark.r());
    }

    #[test]
    fn the_gradient_ends_where_it_should() {
        let p = &Palette::LIGHT;
        assert_eq!(lerp_colour(p.title_start, p.title_end, 0.0), p.title_start);
        assert_eq!(lerp_colour(p.title_start, p.title_end, 1.0), p.title_end);
        // and clamps rather than extrapolating past either end
        assert_eq!(lerp_colour(p.title_start, p.title_end, -1.0), p.title_start);
        assert_eq!(lerp_colour(p.title_start, p.title_end, 9.0), p.title_end);
    }

    #[test]
    fn the_attribution_names_what_the_licence_requires() {
        // CC BY-SA 4.0's one obligation reaching this code. A test because it
        // is the kind of string that gets "tidied" out by someone shortening a
        // dialog, and the obligation does not go away when it does.
        assert!(ATTRIBUTION.contains("CC BY-SA 4.0"));
        assert!(ATTRIBUTION.contains("VileR"));
        assert!(ATTRIBUTION.contains("Nerd Fonts"));
    }
}

#[cfg(test)]
mod toggle_tests {
    use super::*;

    #[test]
    fn selected_buttons_are_pushed_in_and_idle_ones_stand_proud() {
        // The bug this pins: the tab strip had these the wrong way round,
        // while the monitor buttons had them right. Both now call this.
        assert_eq!(toggle_bevel(true), Bevel::Sunken, "selected = pushed in");
        assert_eq!(toggle_bevel(false), Bevel::Raised, "idle = standing proud");
    }

    #[test]
    fn the_face_colour_follows_the_bevel() {
        let p = Palette::LIGHT;
        assert_eq!(toggle_face(true, &p), p.field_bg);
        assert_eq!(toggle_face(false, &p), p.button_face);
    }
}
