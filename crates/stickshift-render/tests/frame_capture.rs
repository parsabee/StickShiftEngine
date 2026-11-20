// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

//! Frame capture for recording: `capture_frame`, `take_captured_frames` and
//! `finish_captures` on the Metal backend. All skip when no Metal device is
//! available.

use stickshift_render::MetalRenderer;
use stickshift_render_api::{FrameRecorder, OverlayQuad};

mod common;

const WIDTH: u32 = 64;
const HEIGHT: u32 = 48;

fn renderer() -> Option<MetalRenderer> {
    let r = MetalRenderer::new_offscreen(WIDTH, HEIGHT);
    if r.is_none() {
        common::no_gpu();
    }
    r
}

/// Draw a frame: its left half one colour, its right another, as HUD quads
/// over the scene.
fn draw(r: &mut MetalRenderer) {
    r.begin_frame();
    let half = WIDTH as f32 / 2.0;
    r.draw_overlay_quad(&OverlayQuad::solid(
        [0.0, 0.0, half, HEIGHT as f32],
        [0.9, 0.2, 0.1, 1.0],
    ));
    r.draw_overlay_quad(&OverlayQuad::solid(
        [half, 0.0, half, HEIGHT as f32],
        [0.1, 0.3, 0.8, 1.0],
    ));
}

#[test]
fn a_captured_frame_is_the_finished_frame_scaled_down() {
    let Some(mut r) = renderer() else { return };
    let (w, h) = (WIDTH / 2, HEIGHT / 2);
    assert!(r.capture_frame(w, h, 1.5), "a capture asked for");
    draw(&mut r);
    r.submit();
    let frame = r.read_pixels().expect("the frame");
    let captured = r.take_captured_frames();
    assert_eq!(captured.len(), 1);
    let capture = &captured[0];
    assert_eq!((capture.width, capture.height, capture.time), (w, h, 1.5));
    assert_eq!(capture.bgra8.len(), (w * h * 4) as usize);
    // At half the size, each captured pixel is the average of the frame's two
    // by two under it, made opaque.
    for y in 0..h as usize {
        for x in 0..w as usize {
            for c in 0..3 {
                let at =
                    |fx: usize, fy: usize| f32::from(frame[(fy * WIDTH as usize + fx) * 4 + c]);
                let (fx, fy) = (2 * x, 2 * y);
                let mean =
                    (at(fx, fy) + at(fx + 1, fy) + at(fx, fy + 1) + at(fx + 1, fy + 1)) / 4.0;
                let got = f32::from(capture.bgra8[(y * w as usize + x) * 4 + c]);
                assert!(
                    (got - mean).abs() <= 1.5,
                    "({x}, {y})[{c}]: {got}, not {mean}"
                );
            }
            assert_eq!(capture.bgra8[(y * w as usize + x) * 4 + 3], 255);
        }
    }
    // The left half the one colour, the right the other: not mirrored.
    let pixel = |x: usize| &capture.bgra8[x * 4..x * 4 + 3];
    assert!(pixel(0)[2] > pixel(0)[0], "the left is red");
    assert!(
        pixel(w as usize - 1)[0] > pixel(w as usize - 1)[2],
        "the right is blue"
    );
}

#[test]
fn a_capture_is_of_the_next_frame_submitted_only() {
    let Some(mut r) = renderer() else { return };
    // Asked for between frames, it is the next frame's.
    assert!(r.capture_frame(16, 16, 0.0));
    draw(&mut r);
    r.submit();
    draw(&mut r);
    r.submit();
    assert_eq!(r.take_captured_frames().len(), 1);
    assert!(r.take_captured_frames().is_empty(), "taken once");
    // A size of nothing is refused.
    assert!(!r.capture_frame(0, 16, 1.0));
}

#[test]
fn captures_not_yet_taken_hold_their_slots_until_they_are() {
    let Some(mut r) = renderer() else { return };
    let mut asked = Vec::new();
    for n in 0..6 {
        if r.capture_frame(16, 12, f64::from(n)) {
            asked.push(f64::from(n));
        }
        draw(&mut r);
        r.submit();
    }
    // One more slot than frames in flight; the rest refused, not waited for.
    assert_eq!(asked.len(), 4, "{asked:?}");
    let taken: Vec<f64> = r.take_captured_frames().iter().map(|f| f.time).collect();
    assert_eq!(taken, asked, "oldest first");
    assert!(r.capture_frame(16, 12, 9.0), "a slot free again");
}

#[test]
fn finishing_waits_for_captures_still_on_the_gpu() {
    let Some(mut r) = renderer() else { return };
    for n in 0..3 {
        assert!(r.capture_frame(32, 24, f64::from(n)));
        draw(&mut r);
        r.submit_async();
    }
    let times: Vec<f64> = r.finish_captures().iter().map(|f| f.time).collect();
    assert_eq!(times, [0.0, 1.0, 2.0]);
}
