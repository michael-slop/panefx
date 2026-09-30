// GENERATED from slop/pkg/theme -- do not edit; edit theme.go and re-run
// go generate ./pkg/theme. This replaces the hand-lifted values win98.rs
// carried (its own comment: "three independent guesses at 'Windows 98
// grey' would not be" one family).

// Copy this file to panefx/src/theme_gen.rs (panefx is a separate Rust
// crate and cannot import a Go module; the copy being GENERATED is what
// keeps it honest -- regenerate and recopy, never hand-edit).

#![allow(dead_code)]

use eframe::egui::Color32;

pub mod light {
    use super::Color32;
    pub const ACCENT: Color32 = Color32::from_rgb(0, 128, 128);
    pub const BAD: Color32 = Color32::from_rgb(168, 0, 0);
    pub const BEVEL_DARK: Color32 = Color32::from_rgb(10, 10, 10);
    pub const BEVEL_LIGHT: Color32 = Color32::from_rgb(223, 223, 223);
    pub const BEVEL_SHADOW: Color32 = Color32::from_rgb(128, 128, 128);
    pub const BEVEL_WHITE: Color32 = Color32::from_rgb(255, 255, 255);
    pub const BLACK: Color32 = Color32::from_rgb(0, 0, 0);
    pub const BONE: Color32 = Color32::from_rgb(236, 233, 221);
    pub const BTN_FACE: Color32 = Color32::from_rgb(192, 192, 192);
    pub const COBALT: Color32 = Color32::from_rgb(16, 132, 208);
    pub const DESKTOP: Color32 = Color32::from_rgb(0, 128, 128);
    pub const FG: Color32 = Color32::from_rgb(34, 34, 34);
    pub const FIELD_BG: Color32 = Color32::from_rgb(255, 255, 255);
    pub const GUTTER_BG: Color32 = Color32::from_rgb(228, 228, 228);
    pub const GUTTER_TEXT: Color32 = Color32::from_rgb(128, 128, 128);
    pub const LAVENDER: Color32 = Color32::from_rgb(111, 106, 133);
    pub const MUTED: Color32 = Color32::from_rgb(74, 74, 74);
    pub const NAVY: Color32 = Color32::from_rgb(0, 0, 128);
    pub const OK: Color32 = Color32::from_rgb(0, 122, 0);
    pub const ORCHID: Color32 = Color32::from_rgb(109, 79, 168);
    pub const PANEL_BG: Color32 = Color32::from_rgb(212, 208, 200);
    pub const TITLE_END: Color32 = Color32::from_rgb(16, 132, 208);
    pub const TITLE_INACTIVE: Color32 = Color32::from_rgb(128, 128, 128);
    pub const TITLE_INACT_END: Color32 = Color32::from_rgb(181, 181, 181);
    pub const TITLE_START: Color32 = Color32::from_rgb(0, 0, 128);
    pub const WARN: Color32 = Color32::from_rgb(138, 109, 0);
    pub const WHITE: Color32 = Color32::from_rgb(255, 255, 255);
    pub const YELLOW: Color32 = Color32::from_rgb(255, 204, 0);
}

pub mod dark {
    use super::Color32;
    pub const ACCENT: Color32 = Color32::from_rgb(98, 230, 112);
    pub const BAD: Color32 = Color32::from_rgb(184, 69, 58);
    pub const BEVEL_DARK: Color32 = Color32::from_rgb(0, 0, 0);
    pub const BEVEL_LIGHT: Color32 = Color32::from_rgb(26, 36, 48);
    pub const BEVEL_SHADOW: Color32 = Color32::from_rgb(7, 11, 16);
    pub const BEVEL_WHITE: Color32 = Color32::from_rgb(38, 51, 63);
    pub const BLACK: Color32 = Color32::from_rgb(0, 0, 0);
    pub const BONE: Color32 = Color32::from_rgb(216, 212, 196);
    pub const BTN_FACE: Color32 = Color32::from_rgb(17, 24, 35);
    pub const COBALT: Color32 = Color32::from_rgb(79, 138, 214);
    pub const DESKTOP: Color32 = Color32::from_rgb(5, 7, 10);
    pub const FG: Color32 = Color32::from_rgb(216, 212, 196);
    pub const FIELD_BG: Color32 = Color32::from_rgb(10, 14, 20);
    pub const GUTTER_BG: Color32 = Color32::from_rgb(10, 14, 20);
    pub const GUTTER_TEXT: Color32 = Color32::from_rgb(92, 100, 112);
    pub const LAVENDER: Color32 = Color32::from_rgb(172, 164, 200);
    pub const MUTED: Color32 = Color32::from_rgb(139, 135, 120);
    pub const NAVY: Color32 = Color32::from_rgb(26, 36, 48);
    pub const OK: Color32 = Color32::from_rgb(98, 230, 112);
    pub const ORCHID: Color32 = Color32::from_rgb(187, 155, 247);
    pub const PANEL_BG: Color32 = Color32::from_rgb(17, 24, 35);
    pub const TITLE_END: Color32 = Color32::from_rgb(26, 36, 48);
    pub const TITLE_INACTIVE: Color32 = Color32::from_rgb(10, 14, 20);
    pub const TITLE_INACT_END: Color32 = Color32::from_rgb(17, 24, 35);
    pub const TITLE_START: Color32 = Color32::from_rgb(10, 14, 20);
    pub const WARN: Color32 = Color32::from_rgb(212, 168, 67);
    pub const WHITE: Color32 = Color32::from_rgb(200, 255, 208);
    pub const YELLOW: Color32 = Color32::from_rgb(212, 168, 67);
}
