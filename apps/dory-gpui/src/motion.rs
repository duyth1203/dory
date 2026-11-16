//! The processing view's motion tokens and curves (`docs/design/processing-view.md` sections 3
//! and 5). Every animation changes only opacity, color, position, or size, and each has a Reduce
//! Motion stand-in that its view picks with `cx.reduce_motion()`. Loops run only while
//! [`looping`] says so: never under Reduce Motion, and never while the window is hidden,
//! occluded, or the app is hidden, so nobody pays for frames nobody sees.

use std::sync::OnceLock;
use std::time::{Duration, Instant};

use gpui_kit::{App, Hsla, Rgba, Window};

/// Fades of single elements.
pub const FAST: Duration = Duration::from_millis(150);
/// Color changes, a line arriving.
pub const BASE: Duration = Duration::from_millis(240);
/// Height changes and the hand-over to the player.
pub const SLOW: Duration = Duration::from_millis(400);
/// Breathing loops.
pub const PULSE: Duration = Duration::from_millis(1600);
/// The waveform's bars arrive left to right this far apart.
pub const STAGGER: Duration = Duration::from_millis(4);

/// Milliseconds on one clock every loop shares, so the dots in the row, the step, and the tab
/// breathe together.
pub fn clock_ms() -> u64 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_millis() as u64
}

/// Whether loops may run in this window now.
pub fn looping(window: &Window, cx: &App) -> bool {
    !cx.reduce_motion() && window.visibility().is_visible()
}

/// How far `over` has run since `since`, 0 to 1, linear. Done (1) when there was no start.
pub fn elapsed(since: Option<Instant>, over: Duration) -> f32 {
    match since {
        Some(at) if !over.is_zero() => (at.elapsed().as_secs_f32() / over.as_secs_f32()).min(1.0),
        _ => 1.0,
    }
}

/// `ease_standard`: cubic-bezier(0.2, 0, 0, 1).
pub fn ease_standard(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    let x = |s: f32| 0.6 * s * (1.0 - s) * (1.0 - s) + s * s * s;
    let (mut lo, mut hi) = (0.0f32, 1.0f32);
    for _ in 0..24 {
        let mid = (lo + hi) * 0.5;
        if x(mid) < t {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    let s = (lo + hi) * 0.5;
    3.0 * s * s - 2.0 * s * s * s
}

fn ease_in_out(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// 0 at the start of the period, 1 halfway, 0 at the end, eased in and out.
fn swell(at_ms: u64, offset: f32) -> f32 {
    let period = PULSE.as_millis() as f32;
    let phase = ((at_ms as f32 / period) + offset).fract();
    ease_in_out(1.0 - (phase * 2.0 - 1.0).abs())
}

/// The reading head's breathing: opacity 1, down to 0.45, back to 1.
pub fn breathe(at_ms: u64) -> f32 {
    1.0 - 0.55 * swell(at_ms, 0.0)
}

/// One of the three dots: opacity 0.3 up to 1 and back, each a third of a period after the last.
pub fn dot(index: usize, at_ms: u64) -> f32 {
    0.3 + 0.7 * swell(at_ms, -(index as f32) / 3.0)
}

/// `from` blended toward `to` by `t`.
pub fn mix(from: Hsla, to: Hsla, t: f32) -> Hsla {
    let (a, b): (Rgba, Rgba) = (from.into(), to.into());
    let t = t.clamp(0.0, 1.0);
    let lerp = |x: f32, y: f32| x + (y - x) * t;
    Rgba {
        r: lerp(a.r, b.r),
        g: lerp(a.g, b.g),
        b: lerp(a.b, b.b),
        a: lerp(a.a, b.a),
    }
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ease_standard_starts_slow_ends_settled_and_never_overshoots() {
        assert!(ease_standard(0.0) < 1e-4);
        assert!((ease_standard(1.0) - 1.0).abs() < 1e-4);
        let mut last = 0.0;
        for i in 1..=20 {
            let v = ease_standard(i as f32 / 20.0);
            assert!(v >= last && v <= 1.0001, "monotonic at {i}: {v}");
            last = v;
        }
        assert!(
            ease_standard(0.5) > 0.7,
            "decelerating: most of the way by half time"
        );
    }

    #[test]
    fn the_loops_stay_inside_their_ranges() {
        let period = PULSE.as_millis() as u64;
        assert!((breathe(0) - 1.0).abs() < 1e-4);
        assert!((breathe(period / 2) - 0.45).abs() < 1e-3);
        for at in (0..period).step_by(50) {
            let b = breathe(at);
            assert!((0.449..=1.0001).contains(&b), "{at}: {b}");
            for i in 0..3 {
                let d = dot(i, at);
                assert!((0.299..=1.0001).contains(&d), "{at}: {d}");
            }
        }
        assert!(
            (dot(0, period / 2) - 1.0).abs() < 1e-3,
            "the first dot peaks at half a period"
        );
        assert!(
            (dot(1, period / 2 + period / 3) - 1.0).abs() < 1e-2,
            "the second a third later"
        );
    }
}
