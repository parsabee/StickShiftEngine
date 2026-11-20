// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

//! Recording what a session shows to a movie: [`Recording`].
//!
//! A recording asks the renderer, before each frame that is due, to capture the
//! frame it is about to present ([`FrameRecorder::capture_frame`]), and after
//! the frame writes whatever captures the GPU has finished to an H.264 movie
//! ([`VideoWriter`]). Nothing waits on the GPU or the encoder while the game
//! runs: a frame whose capture slot or encoder is not ready is skipped, and the
//! movie's frames carry the times they were shown, so it plays at the game's
//! own pace. When the recording stops, the captures still on the GPU are
//! waited for and the movie is finished ([`Session::finish_recording`]).
//!
//! [`FrameRecorder::capture_frame`]: stickshift_render_api::FrameRecorder::capture_frame
//! [`Session::finish_recording`]: crate::Session::finish_recording

use std::path::PathBuf;
use std::time::Duration;

use stickshift_render_api::CapturedFrame;
use stickshift_video::VideoWriter;
pub use stickshift_video::{VideoError, VideoSummary};

use crate::context::Renderer;

/// The frame rate a [`Recording`] takes at most, unless it says otherwise.
pub const DEFAULT_RECORDING_FPS: u32 = 60;

/// A recording of what the game shows to an H.264 movie (`.mp4`): the frames
/// as presented, HUD and all, scaled to [`height`](Self::height) pixels high,
/// at up to [`fps`](Self::fps) frames a second. Start one with
/// [`RunConfig::record`](crate::RunConfig::record), or on a session of your
/// own with [`Session::record`](crate::Session::record).
///
/// Its width follows the view's shape when the first frame is taken (a
/// 16:10 window recorded 720 high is 1152 wide), rounded to even, and stays
/// for the whole movie. No sound is recorded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recording {
    /// Where the movie is written. Any file there is replaced; missing folders
    /// are made.
    pub path: PathBuf,
    /// Its height in pixels: 480 or 720, say. Even, and at least 16.
    pub height: u32,
    /// The most frames a second it takes; it never takes more than are shown.
    pub fps: u32,
}

impl Recording {
    /// A recording to `path`, `height` pixels high, at up to
    /// [`DEFAULT_RECORDING_FPS`] frames a second.
    pub fn new(path: impl Into<PathBuf>, height: u32) -> Self {
        Self {
            path: path.into(),
            height,
            fps: DEFAULT_RECORDING_FPS,
        }
    }

    /// Its width for a view of `aspect` (width over height): its height times
    /// that, to the nearest even pixel, and at least 16.
    #[must_use]
    pub fn width_for(&self, aspect: f32) -> u32 {
        let width = (self.height as f32 * aspect / 2.0).round().max(8.0) as u32 * 2;
        width.max(16)
    }
}

/// A recording in progress.
pub(crate) struct Recorder {
    recording: Recording,
    /// The movie's size, fixed by the view's shape at the first frame taken.
    size: Option<(u32, u32)>,
    /// The movie, from the first frame captured.
    writer: Option<VideoWriter>,
    /// Why it stopped, if it failed.
    failed: Option<VideoError>,
    /// Seconds since the recording started: the time of the frame shown now.
    clock: f64,
    /// When the next frame is due.
    next: f64,
}

impl Recorder {
    pub(crate) fn new(recording: Recording) -> Self {
        Self {
            recording,
            size: None,
            writer: None,
            failed: None,
            clock: 0.0,
            next: 0.0,
        }
    }

    pub(crate) fn recording(&self) -> &Recording {
        &self.recording
    }

    /// Before a frame shown `elapsed` after the one before, in a view of
    /// `aspect`: if a frame is due, ask the renderer to capture it. A frame is
    /// due a frame period after the last one taken, give or take a quarter of
    /// one (so a display's jitter does not halve the rate).
    pub(crate) fn before_frame(
        &mut self,
        renderer: &mut dyn Renderer,
        elapsed: Duration,
        aspect: f32,
    ) {
        self.clock += elapsed.as_secs_f64();
        if self.failed.is_some() {
            return;
        }
        let period = 1.0 / f64::from(self.recording.fps.max(1));
        if self.clock + 0.25 * period < self.next {
            return;
        }
        let height = self.recording.height;
        let (width, height) = *self
            .size
            .get_or_insert_with(|| (self.recording.width_for(aspect), height));
        if renderer.capture_frame(width, height, self.clock) {
            self.next = (self.next + period).max(self.clock + 0.5 * period);
        }
    }

    /// After a frame: write every capture the GPU has finished.
    pub(crate) fn after_frame(&mut self, renderer: &mut dyn Renderer) {
        let frames = renderer.take_captured_frames();
        self.write(frames);
    }

    /// Write `frames` to the movie, starting it at the first.
    fn write(&mut self, frames: Vec<CapturedFrame>) {
        for frame in frames {
            if self.failed.is_some() {
                return;
            }
            let writer = match &mut self.writer {
                Some(writer) => writer,
                None => {
                    match VideoWriter::create(&self.recording.path, frame.width, frame.height) {
                        Ok(writer) => self.writer.insert(writer),
                        Err(e) => {
                            self.failed = Some(e);
                            return;
                        }
                    }
                }
            };
            if let Err(e) = writer.append(&frame.bgra8, frame.time) {
                self.failed = Some(e);
            }
        }
    }

    /// Stop: wait for the captures still on the GPU, write them, and finish
    /// the movie.
    pub(crate) fn finish(
        mut self,
        renderer: &mut dyn Renderer,
    ) -> Result<VideoSummary, VideoError> {
        let frames = renderer.finish_captures();
        self.write(frames);
        if let Some(e) = self.failed {
            return Err(e);
        }
        match self.writer {
            Some(writer) => writer.finish(),
            None => Err(VideoError::Finish("no frame was captured".into())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::driver::Loop;
    use crate::session::{Clock, Session, View};
    use crate::{EngineCtx, Game};
    use stickshift_render_api::{FrameRecorder, NullRenderer};

    /// A game that presents a frame each render, and nothing else.
    struct Presents;

    impl Game for Presents {
        fn init(&mut self, _: &mut EngineCtx) {}
        fn update(&mut self, _: &mut EngineCtx, _: f32) {}
        fn render(&mut self, ctx: &mut EngineCtx) {
            let renderer = ctx.renderer();
            renderer.begin_frame();
            renderer.submit();
        }
    }

    #[test]
    fn a_session_records_the_frames_it_shows_to_a_movie() {
        let dir = std::env::temp_dir().join(format!("stickshift-record-{}", std::process::id()));
        let path = dir.join("run.mp4");
        let (mut game, mut renderer) = (Presents, NullRenderer::new());
        let mut session = Session::new(Loop::new(), None, false);
        let _ = session.resume(View::new(1600, 1000, 2.0), &mut game, &mut renderer);
        let mut recording = Recording::new(&path, 480);
        recording.fps = 30;
        session.record(recording.clone());
        assert_eq!(session.recording(), Some(&recording));
        // Two seconds shown at 60 frames a second: every other one is taken.
        for _ in 0..120 {
            let _ = session.frame(
                Clock::Elapsed(Duration::from_micros(16_667)),
                &mut game,
                &mut renderer,
            );
        }
        let movie = session
            .finish_recording(&mut renderer)
            .expect("recording")
            .expect("a movie");
        assert!(session.recording().is_none(), "stopped");
        // 480 high, as wide as the 16:10 view, from every frame taken.
        assert_eq!((movie.width, movie.height), (768, 480));
        assert!(
            (59..=61).contains(&(movie.frames + movie.dropped)),
            "{movie:?}"
        );
        assert!(movie.frames >= 55, "{movie:?}");
        assert!((movie.seconds - 1.97).abs() < 0.05, "{}", movie.seconds);
        let bytes = std::fs::read(&path).expect("the movie");
        assert_eq!(&bytes[4..8], b"ftyp");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_recording_with_no_frame_taken_says_so() {
        let mut renderer = NullRenderer::new();
        let mut session = Session::new(Loop::new(), None, false);
        session.record(Recording::new("never.mp4", 480));
        let result = session.finish_recording(&mut renderer).expect("recording");
        assert!(matches!(result, Err(VideoError::Finish(_))));
        assert!(session.finish_recording(&mut renderer).is_none(), "once");
        // The null renderer captures what it is asked to, at submit.
        assert!(renderer.capture_frame(32, 16, 0.5));
        renderer.begin_frame();
        renderer.submit();
        let taken = renderer.take_captured_frames();
        assert_eq!(
            (taken[0].width, taken[0].height, taken[0].time),
            (32, 16, 0.5)
        );
    }

    #[test]
    fn a_recordings_width_follows_the_views_shape_to_an_even_pixel() {
        let recording = Recording::new("movie.mp4", 720);
        assert_eq!(recording.width_for(16.0 / 9.0), 1280);
        assert_eq!(recording.width_for(16.0 / 10.0), 1152);
        assert_eq!(recording.width_for(4.0 / 3.0), 960);
        assert_eq!(Recording::new("movie.mp4", 480).width_for(16.0 / 10.0), 768);
        // An odd product rounds to even; a sliver of a view still makes a movie.
        assert_eq!(Recording::new("m.mp4", 482).width_for(1.001) % 2, 0);
        assert_eq!(recording.width_for(0.0), 16);
    }
}
