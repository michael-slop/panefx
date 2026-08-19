//! Render any effect to a PPM, so it can be LOOKED AT before it goes on a
//! desktop.
//!
//! ```text
//! cargo run --release --example effect_preview -- plasma
//! cargo run --release --example effect_preview -- donut 160 60 40
//! ```
//!
//! Arguments: `name [cols] [rows] [warmup-frames]`. The warmup matters for the
//! stateful effects — a starfield one frame after construction is not what a
//! starfield looks like.

use panefx::animation::{self, AsciiAnimation};
use panefx::config::Config;

fn main() {
    let mut args = std::env::args().skip(1);
    let name = args.next().unwrap_or_else(|| "plasma".into());
    let cols: usize = args.next().and_then(|s| s.parse().ok()).unwrap_or(160);
    let rows: usize = args.next().and_then(|s| s.parse().ok()).unwrap_or(60);
    let warmup: usize = args.next().and_then(|s| s.parse().ok()).unwrap_or(30);

    let cfg = Config::default();
    let mut fx = animation::build(&name, cols, rows, 0x5EED_1234, &cfg);
    for _ in 0..warmup {
        fx.step();
    }

    // 8x12 pixels per cell: enough to tell the ramp glyphs apart at a glance
    // without making the file huge.
    let (cw, chh) = (8usize, 12usize);
    let (iw, ih) = (cols * cw, rows * chh);
    let mut buf = vec![0u8; iw * ih * 3];

    // Crude glyph coverage. This is a PREVIEW -- it says "how much ink is in
    // this cell", not "what does this glyph look like". Good enough to judge
    // composition, movement and colour, which is what it is for.
    let coverage = |c: char| -> f32 {
        match c {
            ' ' => 0.0,
            '.' | ',' | '\u{2591}' => 0.25,
            ':' | ';' | '-' | '~' | '\u{2592}' => 0.45,
            '=' | '+' | '!' | '*' | '\u{2593}' => 0.65,
            '#' | '%' | '@' | '$' | '\u{2588}' => 1.0,
            '\u{2580}' | '\u{2584}' | '\u{258c}' | '\u{2590}' => 0.5,
            _ => 0.55,
        }
    };

    for r in 0..rows {
        for c in 0..cols {
            let Some((glyph, colour)) = fx.cell_at(c, r) else {
                continue;
            };
            let a = coverage(glyph);
            if a <= 0.0 {
                continue;
            }
            for y in 0..chh {
                for x in 0..cw {
                    let px = ((r * chh + y) * iw + (c * cw + x)) * 3;
                    buf[px] = (colour.0 as f32 * a) as u8;
                    buf[px + 1] = (colour.1 as f32 * a) as u8;
                    buf[px + 2] = (colour.2 as f32 * a) as u8;
                }
            }
        }
    }

    let out = format!("fx-{name}.ppm");
    let mut bytes = format!("P6\n{iw} {ih}\n255\n").into_bytes();
    bytes.extend_from_slice(&buf);
    std::fs::write(&out, bytes).expect("write ppm");
    println!("{out}  ({cols}x{rows} cells, {warmup} frames warmup)");
}
