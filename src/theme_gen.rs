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
    pub const DESKTOP: Color32 = Color32::from_rgb(0, 128, 128);
    pub const FG: Color32 = Color32::from_rgb(34, 34, 34);
    pub const FIELD_BG: Color32 = Color32::from_rgb(255, 255, 255);
    pub const GUTTER_BG: Color32 = Color32::from_rgb(228, 228, 228);
    pub const GUTTER_TEXT: Color32 = Color32::from_rgb(128, 128, 128);
    pub const MUTED: Color32 = Color32::from_rgb(74, 74, 74);
    pub const NAVY: Color32 = Color32::from_rgb(0, 0, 128);
    pub const OK: Color32 = Color32::from_rgb(0, 122, 0);
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
    pub const ACCENT: Color32 = Color32::from_rgb(79, 214, 190);
    pub const BAD: Color32 = Color32::from_rgb(224, 108, 96);
    pub const BEVEL_DARK: Color32 = Color32::from_rgb(0, 0, 0);
    pub const BEVEL_LIGHT: Color32 = Color32::from_rgb(74, 74, 74);
    pub const BEVEL_SHADOW: Color32 = Color32::from_rgb(26, 26, 26);
    pub const BEVEL_WHITE: Color32 = Color32::from_rgb(110, 110, 110);
    pub const BLACK: Color32 = Color32::from_rgb(0, 0, 0);
    pub const BONE: Color32 = Color32::from_rgb(236, 233, 221);
    pub const BTN_FACE: Color32 = Color32::from_rgb(43, 43, 43);
    pub const DESKTOP: Color32 = Color32::from_rgb(6, 32, 31);
    pub const FG: Color32 = Color32::from_rgb(216, 216, 216);
    pub const FIELD_BG: Color32 = Color32::from_rgb(30, 30, 30);
    pub const GUTTER_BG: Color32 = Color32::from_rgb(42, 42, 42);
    pub const GUTTER_TEXT: Color32 = Color32::from_rgb(124, 124, 124);
    pub const MUTED: Color32 = Color32::from_rgb(154, 154, 154);
    pub const NAVY: Color32 = Color32::from_rgb(79, 79, 79);
    pub const OK: Color32 = Color32::from_rgb(76, 175, 80);
    pub const PANEL_BG: Color32 = Color32::from_rgb(36, 36, 36);
    pub const TITLE_END: Color32 = Color32::from_rgb(94, 94, 94);
    pub const TITLE_INACTIVE: Color32 = Color32::from_rgb(36, 36, 36);
    pub const TITLE_INACT_END: Color32 = Color32::from_rgb(54, 54, 54);
    pub const TITLE_START: Color32 = Color32::from_rgb(58, 58, 58);
    pub const WARN: Color32 = Color32::from_rgb(216, 169, 60);
    pub const WHITE: Color32 = Color32::from_rgb(237, 237, 237);
    pub const YELLOW: Color32 = Color32::from_rgb(255, 204, 0);
}
