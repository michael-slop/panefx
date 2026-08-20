//! Render a strip of `warlockspin` frames, so the rock can be LOOKED at rather
//! than assumed from a passing test.
//!
//! `cargo run --release --example warlock_frames` writes `warlock-strip.ppm`:
//! frames sampled across one full rock cycle, from facing the viewer out to the
//! far extreme and back.

use panefx::animation::AsciiAnimation;
use panefx::warlockspin::{rock, WarlockSpin};

fn main() {
    let (cols, rows) = (44usize, 32usize);
    let (cw, ch) = (8usize, 12usize);
    let step_ms = 40u64;

    // One full cycle at the default rate, sampled evenly. The motion is a
    // smooth rock rather than an irregular sequence, so even sampling is
    // honest here -- unlike the skull, whose wink and jump had to be hunted.
    let rate = 0.25f32;
    let cycle = 1.0 / rate;
    let n = 8usize;
    let want: Vec<f32> = (0..n).map(|i| cycle * i as f32 / n as f32).collect();

    for (i, t) in want.iter().enumerate() {
        eprintln!("frame {i}: t={t:.2}s  cos={:.3}", rock(*t, rate, 0.85));
    }

    let (iw, ih) = (cols * cw * n, rows * ch);
    let mut buf = vec![0u8; iw * ih * 3];

    for (fi, &target) in want.iter().enumerate() {
        // Re-run from zero: the effect is a pure function of elapsed time, so
        // this reproduces the exact frame.
        let mut w = WarlockSpin::new(cols, rows);
        w.set_frame_ms(step_ms);
        let steps = (target / (step_ms as f32 / 1000.0)).round() as usize;
        for _ in 0..steps {
            w.step();
        }
        for r in 0..rows {
            for c in 0..cols {
                let Some((glyph, colour)) = w.cell_at(c, r) else {
                    continue;
                };
                // Glyph density -> coverage, so the PPM approximates what the
                // terminal shows rather than painting every cell solid.
                let a = match glyph {
                    ' ' => 0.0,
                    '.' => 0.30,
                    '+' => 0.55,
                    '*' => 0.75,
                    _ => 1.0,
                };
                if a <= 0.0 {
                    continue;
                }
                for y in 0..ch {
                    for x in 0..cw {
                        let px = ((r * ch + y) * iw + (fi * cols * cw + c * cw + x)) * 3;
                        buf[px] = (colour.0 as f32 * a) as u8;
                        buf[px + 1] = (colour.1 as f32 * a) as u8;
                        buf[px + 2] = (colour.2 as f32 * a) as u8;
                    }
                }
            }
        }
    }

    let mut out = format!("P6\n{iw} {ih}\n255\n").into_bytes();
    out.extend_from_slice(&buf);
    std::fs::write("warlock-strip.ppm", out).expect("write ppm");
    println!("warlock-strip.ppm  ({n} frames at {cols}x{rows})");
}
