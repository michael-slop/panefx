//! Render a strip of `skullspin` frames, so the spin and the jump can be
//! LOOKED at rather than assumed from a passing test.
//!
//! `cargo run --release --example skull_frames` writes `skull-strip.ppm`: a row
//! of frames sampled across a few seconds. Frames are chosen to land on the
//! jump rather than at fixed intervals, because on a fixed interval an
//! irregular event is exactly what you miss. (This is also how the wink's
//! removal was confirmed by eye rather than by a green test.)

use panefx::animation::AsciiAnimation;
use panefx::skullspin::{sequence, Act, SkullSpin};

fn main() {
    let (cols, rows) = (46usize, 26usize);
    let (cw, ch) = (8usize, 12usize);
    let step_ms = 40u64;

    // Walk time and collect one frame of each ACT, plus a couple of plain
    // spins. Sampling at fixed intervals would miss them: the sequence is
    // deliberately irregular, which is the whole point of it.
    let mut want: Vec<f32> = vec![0.0, 0.4];
    let mut t = 0.0f32;
    let mut got_jump = false;
    while t < 90.0 && !got_jump {
        let (act, phase, _) = sequence(t, 4.0, 0x5EED);
        match act {
            Act::Jump if !got_jump && phase > 0.1 => {
                // The whole arc: crouch, rise, peak, fall, land.
                for d in [-0.10f32, 0.0, 0.16, 0.30, 0.46] {
                    want.push((t + d).max(0.0));
                }
                got_jump = true;
            }
            _ => {}
        }
        t += step_ms as f32 / 1000.0;
    }
    want.sort_by(|a, b| a.partial_cmp(b).unwrap());
    eprintln!("jump captured: {got_jump}   frames: {}", want.len());

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
