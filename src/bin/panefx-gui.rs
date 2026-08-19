//! panefx-gui — the Windows 98 control panel.
//!
//! STEP 1: a chrome proof. Draws a title bar, a raised panel, a sunken well and
//! one of each bevel style, so the look can be compared against slopkit before
//! any of the app is built on top of it. If the ring does not read as Win98
//! here, that is worth knowing before three tabs and a preview pipeline are
//! sitting on it.
//!
//! See `src/win98.rs` for where every value comes from.

#![windows_subsystem = "windows"]

use eframe::egui::{self, Color32, CornerRadius, Rect, Vec2};
use panefx::win98::{self, Bevel, Palette};

fn main() -> eframe::Result<()> {
    let opts = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([720.0, 480.0])
            .with_min_inner_size([420.0, 320.0])
            // The app draws its own Win98 title bar; the OS one would be a
            // second, differently-styled bar stacked above it.
            .with_decorations(false)
            .with_title("panefx"),
        ..Default::default()
    };
    eframe::run_native(
        "panefx",
        opts,
        Box::new(|cc| {
            win98::apply_theme(&cc.egui_ctx, &Palette::LIGHT);
            Ok(Box::new(Proof::default()))
        }),
    )
}

#[derive(Default)]
struct Proof {
    dark: bool,
}

impl eframe::App for Proof {
    // eframe 0.36 hands the app a `Ui` rather than a `Context` -- the frame is
    // already begun and the central panel already allocated.
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let p = if self.dark { Palette::DARK } else { Palette::LIGHT };
        let ctx = ui.ctx().clone();
        {
            {
                let full = ui.max_rect();
                let painter = ui.painter().clone();

                // The window itself: a raised panel filling the viewport, with
                // the title bar inside its bevel.
                win98::bevel(&painter, full, Bevel::Raised, &p, Some(p.button_face));

                let inner = full.shrink(win98::BEVEL_THICKNESS);
                let bar = Rect::from_min_size(
                    inner.min,
                    Vec2::new(inner.width(), win98::TITLE_BAR_HEIGHT),
                );
                win98::title_bar(&painter, bar, "panefx — chrome proof", true, &p);

                // Dragging the title bar moves the window, since there is no OS
                // titlebar to do it.
                let drag = ui.interact(bar, ui.id().with("titlebar"), egui::Sense::click_and_drag());
                if drag.is_pointer_button_down_on() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
                }

                let body = Rect::from_min_max(
                    egui::pos2(inner.min.x + 8.0, bar.max.y + 8.0),
                    egui::pos2(inner.max.x - 8.0, inner.max.y - 8.0),
                );
                let mut child = ui.new_child(egui::UiBuilder::new().max_rect(body));
                self.body(&mut child, &p);
            }
        }
    }
}

impl Proof {
    fn body(&mut self, ui: &mut egui::Ui, p: &Palette) {
        ui.horizontal(|ui| {
            if ui.button(if self.dark { "light" } else { "dark" }).clicked() {
                self.dark = !self.dark;
                win98::apply_theme(ui.ctx(), if self.dark { &Palette::DARK } else { &Palette::LIGHT });
            }
            ui.label("← the palette swaps at runtime");
        });

        ui.add_space(8.0);

        // One of each bevel style, side by side, so they can be told apart.
        ui.horizontal(|ui| {
            for (style, name) in [
                (Bevel::Raised, "Raised"),
                (Bevel::Sunken, "Sunken"),
                (Bevel::Thin, "Thin"),
            ] {
                let (rect, _) = ui.allocate_exact_size(Vec2::new(150.0, 70.0), egui::Sense::hover());
                let fill = if style == Bevel::Sunken { p.field_bg } else { p.button_face };
                win98::bevel(ui.painter(), rect, style, p, Some(fill));
                ui.painter().text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    name,
                    egui::FontId::new(win98::size::TEXT, egui::FontFamily::Monospace),
                    p.text,
                );
            }
        });

        ui.add_space(8.0);

        // A sunken well with the font in it, at the sizes the app will use.
        let (well, _) = ui.allocate_exact_size(
            Vec2::new(ui.available_width(), 120.0),
            egui::Sense::hover(),
        );
        win98::bevel(ui.painter(), well, Bevel::Sunken, p, Some(p.field_bg));
        let mut y = well.min.y + 8.0;
        for (size, label) in [
            (win98::size::TEXT, "body 14 — the quick brown fox"),
            (win98::TITLE_TEXT_SIZE, "caption 13 — ABCDEFGHIJ 0123456789"),
            (11.0, "status 11 — \u{2588}\u{2593}\u{2592}\u{2591} block glyphs render"),
        ] {
            ui.painter().text(
                egui::pos2(well.min.x + 8.0, y),
                egui::Align2::LEFT_TOP,
                label,
                egui::FontId::new(size, egui::FontFamily::Monospace),
                p.text,
            );
            y += size + 8.0;
        }

        ui.add_space(8.0);

        // The colour swatch strip: proves the palette is what the tests pin.
        ui.horizontal_wrapped(|ui| {
            for (name, c) in [
                ("desktop", p.desktop),
                ("face", p.button_face),
                ("navy", p.navy),
                ("field", p.field_bg),
                ("panel", p.panel_bg),
                ("bone", p.bone),
                ("yellow", p.yellow),
                ("ok", p.ok),
                ("bad", p.bad),
                ("warn", p.warn),
            ] {
                // Height covers the swatch AND its caption: allocating only the
                // swatch let the label overflow into whatever was drawn below.
                let (cell, _) = ui.allocate_exact_size(Vec2::new(52.0, 48.0), egui::Sense::hover());
                let r = Rect::from_min_size(cell.min, Vec2::new(52.0, 34.0));
                win98::bevel(ui.painter(), r, Bevel::Thin, p, Some(c));
                ui.painter().text(
                    egui::pos2(r.center().x, r.max.y + 2.0),
                    egui::Align2::CENTER_TOP,
                    name,
                    egui::FontId::new(9.0, egui::FontFamily::Monospace),
                    p.muted,
                );
            }
        });

        ui.add_space(20.0);
        ui.horizontal(|ui| {
            if ui.button("close").clicked() {
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
            }
            ui.label(egui::RichText::new("drag the title bar to move").color(p.muted));
        });

        // Keep the swatch labels from being clipped by the panel edge.
        let _ = CornerRadius::ZERO;
        let _: Color32 = p.black;
    }
}
