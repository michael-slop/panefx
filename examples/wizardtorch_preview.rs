//! Render `wizardtorch` frames to PPM, so the effect can be LOOKED AT without
//! putting it on the desktop first.
//!
//! `cargo run --example wizardtorch_preview` writes `wt-000.ppm` .. `wt-003.ppm`
//! into the current directory, sampled far enough apart in time to show the
//! flicker actually moving.

use panefx::animation::AsciiAnimation;
use panefx::wizardtorch::WizardTorch;

/// Coverage of each glyph, for the preview's crude "draw a cell" step.
fn coverage(ch: char) -> (f32, u8) {
    match ch {
        '\u{2591}' => (0.25, 0),
        '\u{2592}' => (0.50, 0),
        '\u{2593}' => (0.75, 0),
        '\u{2588}' => (1.00, 0),
        '\u{2580}' => (1.00, 1), // top half
        '\u{2584}' => (1.00, 2), // bottom half
        '\u{258c}' => (1.00, 3), // left half
        '\u{2590}' => (1.00, 4), // right half
        _ => (0.0, 0),
    }
}

fn main() {
    // A LANDSCAPE grid on purpose: the whole point of the fit work is that a
    // portrait drawing has to sit sensibly on a wide monitor.
    let (cols, rows) = (160usize, 60usize);
    let (cw, ch) = (8usize, 12usize);
    let mut w = WizardTorch::new(cols, rows);
    w.set_frame_ms(100);
    // Override from argv so all three fit modes can be eyeballed without an
    // edit-rebuild cycle: `cargo run --example wizardtorch_preview -- cover`.
    if let Some(fit) = std::env::args().nth(1) {
        use panefx::animation::ParamValue;
        assert!(w.set_param("fit", &ParamValue::Text { v: fit.clone() }), "bad fit {fit}");
    }

    for frame in 0..4 {
        // 12 steps at 100ms = 1.2s between saved frames, which is long enough
        // for the slow wander to have visibly moved.
        for _ in 0..12 {
            w.step();
        }
        let (iw, ih) = (cols * cw, rows * ch);
        let mut buf = vec![0u8; iw * ih * 3];
        for r in 0..rows {
            for c in 0..cols {
                let Some((glyph, colour)) = w.cell_at(c, r) else {
                    continue;
                };
                let (cov, half) = coverage(glyph);
                if cov == 0.0 {
                    continue;
                }
                for y in 0..ch {
                    for x in 0..cw {
                        let inside = match half {
                            1 => y < ch / 2,
                            2 => y >= ch / 2,
                            3 => x < cw / 2,
                            4 => x >= cw / 2,
                            _ => true,
                        };
                        if !inside {
                            continue;
                        }
                        let a = if half == 0 { cov } else { 1.0 };
                        let px = ((r * ch + y) * iw + (c * cw + x)) * 3;
                        buf[px] = (colour.0 as f32 * a) as u8;
                        buf[px + 1] = (colour.1 as f32 * a) as u8;
                        buf[px + 2] = (colour.2 as f32 * a) as u8;
                    }
                }
            }
        }
        let name = format!("wt-{frame:03}.ppm");
        let mut out = format!("P6\n{iw} {ih}\n255\n").into_bytes();
        out.extend_from_slice(&buf);
        std::fs::write(&name, out).expect("write ppm");
        println!("{name}");
    }
}
