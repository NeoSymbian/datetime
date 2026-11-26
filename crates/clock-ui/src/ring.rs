// SPDX-License-Identifier: GPL-3.0-only
//! The timer's ProgressRing as pixels: the phone's Slint (software renderer, no path feature) can't stroke an arc. A
//! `track` circle with the share `fraction` in `colour` over it, clockwise from 12 o'clock, anti-aliased across the
//! stroke (the arc's two ends are cut square, as the board's SVG stroke).

use alloc::vec::Vec;
use datetime_core::analog::{Rgba, sin_cos};
use slint::{Image, Rgba8Pixel, SharedPixelBuffer};

/// `size` × `size` pixels, the stroke `stroke` px wide touching the square's edge.
#[must_use]
pub fn draw(size: usize, stroke: f32, fraction: f32, colour: [u8; 3], track: [u8; 3]) -> Vec<Rgba> {
    let c = size as f32 / 2.0;
    let half = stroke / 2.0;
    let r = c - half;
    let (outer, inner) = (r + half + 0.5, (r - half - 0.5).max(0.0));
    let f = fraction.clamp(0.0, 1.0);
    // the arc's end, as a direction from the centre (y down)
    let (s, k) = sin_cos(f * 360.0);
    let end = (s, -k);
    let mut px = alloc::vec![[0u8; 4]; size * size];
    for y in 0..size {
        for x in 0..size {
            let (dx, dy) = (x as f32 + 0.5 - c, y as f32 + 0.5 - c);
            let d2 = dx * dx + dy * dy;
            if d2 > outer * outer || d2 < inner * inner {
                continue;
            }
            let cover = (half + 0.5 - (sqrt(d2) - r).abs()).clamp(0.0, 1.0);
            let ink = if swept((dx, dy), end, f) {
                colour
            } else {
                track
            };
            px[y * size + x] = [ink[0], ink[1], ink[2], (cover * 255.0 + 0.5) as u8];
        }
    }
    px
}

/// The ring as a Slint image ([`draw`]).
#[must_use]
pub fn image(size: usize, stroke: f32, fraction: f32, colour: [u8; 3], track: [u8; 3]) -> Image {
    let mut buf = SharedPixelBuffer::<Rgba8Pixel>::new(size as u32, size as u32);
    let (dst, _) = buf.make_mut_bytes().as_chunks_mut::<4>();
    dst.copy_from_slice(&draw(size, stroke, fraction, colour, track));
    Image::from_rgba8(buf)
}

/// Whether `p` lies in the clockwise sweep from 12 o'clock to `end` (`f` of the way round). On screen (y down) a
/// clockwise turn from a to b has a positive cross product a × b.
fn swept(p: (f32, f32), end: (f32, f32), f: f32) -> bool {
    let cross = |a: (f32, f32), b: (f32, f32)| a.0 * b.1 - a.1 * b.0;
    if f >= 1.0 {
        true
    } else if f <= 0.0 {
        false
    } else if f <= 0.5 {
        // right of 12 o'clock and not past the end
        p.0 >= 0.0 && cross(p, end) >= 0.0
    } else {
        // all but the part between the end and 12 o'clock, which is left of the centre line
        !(p.0 < 0.0 && cross(end, p) > 0.0)
    }
}

/// Newton's square root (no libm here); plenty for edge distances.
fn sqrt(v: f32) -> f32 {
    if v <= 0.0 {
        return 0.0;
    }
    let mut x = f32::from_bits((v.to_bits() >> 1) + 0x1fbd_1df5);
    for _ in 0..3 {
        x = 0.5 * (x + v / x);
    }
    x
}

#[cfg(test)]
mod tests {
    use super::*;

    const S: usize = 100;

    fn at(px: &[Rgba], x: usize, y: usize) -> Rgba {
        px[y * S + x]
    }

    #[test]
    fn the_sweep_runs_clockwise_from_twelve() {
        let (on, off) = ([255, 0, 0], [0, 0, 255]);
        // a quarter: 3 o'clock's side is past the end, 1:30 is inside
        let q = draw(S, 10.0, 0.25, on, off);
        assert_eq!(at(&q, 50, 2)[..3], on, "12 o'clock");
        assert_eq!(at(&q, 84, 15)[..3], on, "1:30");
        assert_eq!(at(&q, 50, 97)[..3], off, "6 o'clock");
        assert_eq!(at(&q, 2, 50)[..3], off, "9 o'clock");
        // three quarters: 9 o'clock in, 10:30 out
        let t = draw(S, 10.0, 0.75, on, off);
        assert_eq!(at(&t, 50, 97)[..3], on, "6 o'clock");
        assert_eq!(at(&t, 15, 15)[..3], off, "10:30");
        // full and empty
        assert_eq!(at(&draw(S, 10.0, 1.0, on, off), 15, 15)[..3], on);
        assert_eq!(at(&draw(S, 10.0, 0.0, on, off), 84, 15)[..3], off);
    }

    #[test]
    fn only_the_stroke_is_painted() {
        let px = draw(S, 10.0, 0.5, [255; 3], [9; 3]);
        assert_eq!(at(&px, 50, 50)[3], 0, "the centre stays clear");
        assert_eq!(at(&px, 0, 0)[3], 0, "the corner stays clear");
        assert_eq!(at(&px, 50, 5)[3], 255, "inside the stroke: opaque");
    }
}
