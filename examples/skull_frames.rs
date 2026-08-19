//! Render a strip of `skullspin` frames, so the spin, wink and jump can be
//! LOOKED at rather than assumed from a passing test.
//!
//! `cargo run --release --example skull_frames` writes `skull-strip.ppm`: a row
//! of frames sampled across a few seconds. Frames are chosen to land on a wink
//! and a jump rather than at fixed intervals, because on a fixed interval an
//! irregular event is exactly what you miss.

use panefx::animation::AsciiAnimation;
use panefx::skullspin::{jump_phase, wink_phase, SkullSpin};

fn main() {
    let (cols, rows) = (46usize, 26usize);
    let (cw, ch) = (8usize, 12usize);
    let step_ms = 40u64;

    // Walk time and collect the interesting moments: a plain frame, one
    // mid-wink, and several across one jump.
    let mut want: Vec<f32> = vec![0.0, 0.35, 0.7];
    let mut t = 0.0f32;
    let (mut got_wink, mut got_jump) = (false, false);
    while t < 40.0 && !(got_wink && got_jump) {
        if !got_wink {
            if let Some((p, _)) = wink_phase(t, 5.0, 0xBEEF) {
                if p > 0.4 && p < 0.6 {
                    want.push(t);
                    got_wink = true;
                }
            }
        }
        if !got_jump {
            let (lift, _) = jump_phase(t, 9.0, 0x5EED, rows as f32 * 0.55);
            if lift > 1.0 {
                // The whole arc: crouch, rise, peak, fall, land.
                for d in [-0.16f32, -0.05, 0.0, 0.14, 0.28] {
                    want.push((t + d).max(0.0));
                }
                got_jump = true;
            }
        }
        t += step_ms as f32 / 1000.0;
    }
    want.sort_by(|a, b| a.partial_cmp(b).unwrap());
    eprintln!(
        "wink captured: {got_wink}   jump captured: {got_jump}   frames: {}",
        want.len()
    );

    let n = want.len();
    let (iw, ih) = (cols * cw * n, rows * ch);
    let mut buf = vec![0u8; iw * ih * 3];

    for (fi, &target) in want.iter().enumerate() {
        // Re-run from zero to the target: the effect is a pure function of
        // elapsed time, so this reproduces the exact frame.
        let mut s = SkullSpin::new(cols, rows);
        s.set_frame_ms(step_ms);
        let steps = (target / (step_ms as f32 / 1000.0)).round() as usize;
        for _ in 0..steps {
            s.step();
        }
        for r in 0..rows {
            for c in 0..cols {
                let Some((glyph, colour)) = s.cell_at(c, r) else {
                    continue;
                };
                let a = match glyph {
                    ' ' => 0.0,
                    '.' => 0.30,
                    ':' => 0.42,
                    '=' => 0.54,
                    '+' => 0.66,
                    '*' => 0.78,
                    '#' => 0.88,
                    '%' => 0.94,
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
    std::fs::write("skull-strip.ppm", out).expect("write ppm");
    println!("skull-strip.ppm  ({n} frames at {cols}x{rows})");
}
