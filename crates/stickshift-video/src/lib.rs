// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

//! Video recording for Stick Shift Engine: frames written to an H.264 movie.
//!
//! [`VideoWriter`] takes frames as BGRA8 pixels, each stamped with its time, and
//! writes them to an `.mp4` file (H.264 in an MPEG-4 container) as they come.
//! It is what `stickshift-core`'s recording option feeds with the frames the
//! renderer captures; it knows nothing about where they came from, so anything
//! that has pixels can record them.
//!
//! # AVFoundation
//!
//! On Apple platforms the writer is an `AVAssetWriter`: the system's own H.264
//! encoder, in hardware where there is one, and its own MPEG-4 muxer. Nothing is
//! bundled and nothing runs outside the process. Elsewhere [`VideoWriter::create`]
//! returns [`VideoError::Unsupported`].
//!
//! # Real time
//!
//! A game records while it runs, so the writer never holds the game up for
//! long. The encoder takes frames in real time; when it is not ready for the
//! next one within [`READY_PATIENCE`], that frame is **dropped** rather than
//! waited for, and [`VideoWriter::dropped`] counts it. A frame's time is when
//! it was shown, so a movie with dropped frames still plays at the game's own
//! pace. Times must rise: a frame stamped no later than the one before it is
//! dropped too.
//!
//! # Finishing
//!
//! A movie is only playable once it is finished: [`VideoWriter::finish`] waits
//! for the encoder to write out what it holds and close the file. A writer
//! dropped without finishing leaves an unplayable file behind.
#![deny(missing_docs)]

use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// How long [`VideoWriter::append`] waits for the encoder to be ready for a
/// frame before dropping it.
pub const READY_PATIENCE: Duration = Duration::from_millis(8);

/// How long [`VideoWriter::finish`] waits for the encoder to write the movie
/// out and close it.
pub const FINISH_PATIENCE: Duration = Duration::from_secs(30);

/// The units a frame's time is written in: ticks a second.
const TIMESCALE: i32 = 60_000;

/// Why a movie could not be written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VideoError {
    /// This platform has no encoder the writer can use.
    Unsupported,
    /// The movie's size is not one an H.264 encoder takes: each side even
    /// and at least 16.
    BadSize {
        /// The width asked for.
        width: u32,
        /// The height asked for.
        height: u32,
    },
    /// A frame's pixels were not `width * height * 4` bytes.
    BadFrame {
        /// The bytes a frame must have.
        expected: usize,
        /// The bytes it had.
        got: usize,
    },
    /// The movie file could not be started (the path, or the encoder).
    Create(String),
    /// A frame could not be written.
    Write(String),
    /// The movie could not be finished.
    Finish(String),
}

impl fmt::Display for VideoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported => write!(f, "video recording is not supported on this platform"),
            Self::BadSize { width, height } => write!(
                f,
                "a movie {width}x{height} cannot be encoded: each side must be even and at least 16"
            ),
            Self::BadFrame { expected, got } => {
                write!(f, "a frame must be {expected} bytes of BGRA8, not {got}")
            }
            Self::Create(why) => write!(f, "could not start the movie: {why}"),
            Self::Write(why) => write!(f, "could not write a frame: {why}"),
            Self::Finish(why) => write!(f, "could not finish the movie: {why}"),
        }
    }
}

impl std::error::Error for VideoError {}

/// What a finished movie holds.
#[derive(Debug, Clone, PartialEq)]
pub struct VideoSummary {
    /// Where it was written.
    pub path: PathBuf,
    /// Its width in pixels.
    pub width: u32,
    /// Its height in pixels.
    pub height: u32,
    /// The frames written into it.
    pub frames: u64,
    /// The frames handed to it but dropped (the encoder not ready, or out of
    /// time order).
    pub dropped: u64,
    /// From its first frame's time to its last's, in seconds.
    pub seconds: f64,
}

/// An H.264 movie being written, a frame at a time. See the
/// [crate docs](crate).
pub struct VideoWriter {
    inner: platform::Writer,
    path: PathBuf,
    width: u32,
    height: u32,
    frames: u64,
    dropped: u64,
    /// The first frame's time, and the last written's, in ticks.
    first: Option<i64>,
    last: Option<i64>,
}

impl fmt::Debug for VideoWriter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("VideoWriter")
            .field("path", &self.path)
            .field("width", &self.width)
            .field("height", &self.height)
            .field("frames", &self.frames)
            .field("dropped", &self.dropped)
            .finish_non_exhaustive()
    }
}

impl VideoWriter {
    /// Start a movie `width` × `height` pixels at `path`, replacing any file
    /// there. Each side must be even and at least 16. The bit rate suits the
    /// size: about six megabits a second at 1280 × 720.
    ///
    /// # Errors
    ///
    /// [`VideoError::BadSize`] for a size the encoder cannot take,
    /// [`VideoError::Unsupported`] off Apple platforms, and
    /// [`VideoError::Create`] if the file or the encoder cannot be started.
    pub fn create(path: impl AsRef<Path>, width: u32, height: u32) -> Result<Self, VideoError> {
        if width < 16 || height < 16 || !width.is_multiple_of(2) || !height.is_multiple_of(2) {
            return Err(VideoError::BadSize { width, height });
        }
        let path = path.as_ref().to_path_buf();
        let absolute = if path.is_absolute() {
            path.clone()
        } else {
            std::env::current_dir()
                .map_err(|e| VideoError::Create(e.to_string()))?
                .join(&path)
        };
        if let Some(dir) = absolute.parent() {
            std::fs::create_dir_all(dir).map_err(|e| VideoError::Create(e.to_string()))?;
        }
        match std::fs::remove_file(&absolute) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(VideoError::Create(e.to_string())),
        }
        let inner = platform::Writer::create(&absolute, width, height, bit_rate(width, height))?;
        Ok(Self {
            inner,
            path: absolute,
            width,
            height,
            frames: 0,
            dropped: 0,
            first: None,
            last: None,
        })
    }

    /// Write one frame, shown `seconds` into the movie: `width * height * 4`
    /// bytes of BGRA8, rows top first, tightly packed. Returns whether it was
    /// written; it is dropped (and counted) when the encoder is not ready for it
    /// in time or it is no later than the frame before.
    ///
    /// # Errors
    ///
    /// [`VideoError::BadFrame`] for pixels of the wrong size, and
    /// [`VideoError::Write`] if the encoder fails (the movie is then lost).
    pub fn append(&mut self, bgra8: &[u8], seconds: f64) -> Result<bool, VideoError> {
        let expected = self.width as usize * self.height as usize * 4;
        if bgra8.len() != expected {
            return Err(VideoError::BadFrame {
                expected,
                got: bgra8.len(),
            });
        }
        let ticks = (seconds * f64::from(TIMESCALE)).round() as i64;
        if self.last.is_some_and(|last| ticks <= last) {
            self.dropped += 1;
            return Ok(false);
        }
        let first = *self.first.get_or_insert(ticks);
        if !self
            .inner
            .append(bgra8, self.width, self.height, ticks - first)?
        {
            self.dropped += 1;
            return Ok(false);
        }
        self.last = Some(ticks);
        self.frames += 1;
        Ok(true)
    }

    /// The frames written so far.
    #[must_use]
    pub fn frames(&self) -> u64 {
        self.frames
    }

    /// The frames dropped so far.
    #[must_use]
    pub fn dropped(&self) -> u64 {
        self.dropped
    }

    /// The movie's size in pixels.
    #[must_use]
    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Finish the movie: wait (up to [`FINISH_PATIENCE`]) for the encoder to
    /// write out every frame it holds and close the file.
    ///
    /// # Errors
    ///
    /// [`VideoError::Finish`] if the encoder fails or does not finish in time.
    pub fn finish(self) -> Result<VideoSummary, VideoError> {
        self.inner.finish()?;
        let ticks = match (self.first, self.last) {
            (Some(first), Some(last)) => last - first,
            _ => 0,
        };
        Ok(VideoSummary {
            path: self.path,
            width: self.width,
            height: self.height,
            frames: self.frames,
            dropped: self.dropped,
            seconds: ticks as f64 / f64::from(TIMESCALE),
        })
    }
}

/// The average bit rate a movie `width` × `height` is encoded at: a tenth of a
/// bit per pixel per frame at sixty frames a second, and never under a megabit.
fn bit_rate(width: u32, height: u32) -> u32 {
    let bits = f64::from(width) * f64::from(height) * 60.0 * 0.1;
    bits.max(1_000_000.0) as u32
}

#[cfg(target_vendor = "apple")]
mod platform {
    //! The writer on Apple platforms: an `AVAssetWriter`.

    use std::path::Path;
    use std::ptr::NonNull;
    use std::sync::mpsc;
    use std::time::Instant;

    use block2::RcBlock;
    use objc2::rc::{autoreleasepool, Retained};
    use objc2::runtime::AnyObject;
    use objc2_av_foundation::{
        AVAssetWriter, AVAssetWriterInput, AVAssetWriterInputPixelBufferAdaptor,
        AVAssetWriterStatus, AVFileTypeMPEG4, AVMediaTypeVideo, AVVideoAverageBitRateKey,
        AVVideoCodecKey, AVVideoCodecTypeH264, AVVideoCompressionPropertiesKey, AVVideoHeightKey,
        AVVideoMaxKeyFrameIntervalKey, AVVideoWidthKey,
    };
    use objc2_core_foundation::{CFRetained, CFString};
    use objc2_core_media::CMTime;
    use objc2_core_video::{
        kCVPixelBufferHeightKey, kCVPixelBufferPixelFormatTypeKey, kCVPixelBufferWidthKey,
        kCVPixelFormatType_32BGRA, kCVReturnSuccess, CVPixelBuffer, CVPixelBufferGetBaseAddress,
        CVPixelBufferGetBytesPerRow, CVPixelBufferLockBaseAddress, CVPixelBufferLockFlags,
        CVPixelBufferPool, CVPixelBufferUnlockBaseAddress,
    };
    use objc2_foundation::{NSDictionary, NSNumber, NSString, NSURL};

    use super::{VideoError, FINISH_PATIENCE, READY_PATIENCE, TIMESCALE};

    /// The asset writer, its one video input, and the adaptor that takes
    /// pixel buffers into it.
    pub(crate) struct Writer {
        writer: Retained<AVAssetWriter>,
        input: Retained<AVAssetWriterInput>,
        adaptor: Retained<AVAssetWriterInputPixelBufferAdaptor>,
    }

    /// A `CFString` constant as the `NSString` it is toll-free bridged to.
    fn ns(key: &CFString) -> &NSString {
        // SAFETY: `CFString` and `NSString` are toll-free bridged: the same
        // object, so a reference to one is a reference to the other.
        unsafe { &*std::ptr::from_ref(key).cast::<NSString>() }
    }

    /// An AVFoundation `NSString` constant, which the framework defines.
    fn constant(
        key: Option<&'static NSString>,
        name: &str,
    ) -> Result<&'static NSString, VideoError> {
        key.ok_or_else(|| VideoError::Create(format!("AVFoundation lacks {name}")))
    }

    /// A dictionary of `keys` to `values`, for AVFoundation's settings.
    fn dictionary(
        keys: &[&NSString],
        values: &[&AnyObject],
    ) -> Retained<NSDictionary<NSString, AnyObject>> {
        NSDictionary::from_slices(keys, values)
    }

    impl Writer {
        pub(crate) fn create(
            path: &Path,
            width: u32,
            height: u32,
            bit_rate: u32,
        ) -> Result<Self, VideoError> {
            autoreleasepool(|_| {
                // SAFETY: every call below is an AVFoundation method or
                // constant used as documented, on objects this function holds,
                // with arguments of the types the bindings declare.
                unsafe {
                    let path = NSString::from_str(&path.to_string_lossy());
                    let url = NSURL::fileURLWithPath(&path);
                    let file_type = constant(AVFileTypeMPEG4, "AVFileTypeMPEG4")?;
                    let writer = AVAssetWriter::assetWriterWithURL_fileType_error(&url, file_type)
                        .map_err(|e| VideoError::Create(e.localizedDescription().to_string()))?;

                    let compression = dictionary(
                        &[
                            constant(AVVideoAverageBitRateKey, "AVVideoAverageBitRateKey")?,
                            constant(
                                AVVideoMaxKeyFrameIntervalKey,
                                "AVVideoMaxKeyFrameIntervalKey",
                            )?,
                        ],
                        &[
                            NSNumber::new_u32(bit_rate).as_ref(),
                            NSNumber::new_u32(60).as_ref(),
                        ],
                    );
                    let settings = dictionary(
                        &[
                            constant(AVVideoCodecKey, "AVVideoCodecKey")?,
                            constant(AVVideoWidthKey, "AVVideoWidthKey")?,
                            constant(AVVideoHeightKey, "AVVideoHeightKey")?,
                            constant(
                                AVVideoCompressionPropertiesKey,
                                "AVVideoCompressionPropertiesKey",
                            )?,
                        ],
                        &[
                            constant(AVVideoCodecTypeH264, "AVVideoCodecTypeH264")?.as_ref(),
                            NSNumber::new_u32(width).as_ref(),
                            NSNumber::new_u32(height).as_ref(),
                            compression.as_ref(),
                        ],
                    );
                    let media = constant(AVMediaTypeVideo, "AVMediaTypeVideo")?;
                    let input = AVAssetWriterInput::assetWriterInputWithMediaType_outputSettings(
                        media,
                        Some(&settings),
                    );
                    input.setExpectsMediaDataInRealTime(true);

                    let attributes = dictionary(
                        &[
                            ns(kCVPixelBufferPixelFormatTypeKey),
                            ns(kCVPixelBufferWidthKey),
                            ns(kCVPixelBufferHeightKey),
                        ],
                        &[
                            NSNumber::new_u32(kCVPixelFormatType_32BGRA).as_ref(),
                            NSNumber::new_u32(width).as_ref(),
                            NSNumber::new_u32(height).as_ref(),
                        ],
                    );
                    let adaptor = AVAssetWriterInputPixelBufferAdaptor::assetWriterInputPixelBufferAdaptorWithAssetWriterInput_sourcePixelBufferAttributes(
                        &input,
                        Some(&attributes),
                    );

                    if !writer.canAddInput(&input) {
                        return Err(VideoError::Create("the video input was refused".into()));
                    }
                    writer.addInput(&input);
                    if !writer.startWriting() {
                        return Err(VideoError::Create(failure(&writer)));
                    }
                    writer.startSessionAtSourceTime(CMTime::new(0, TIMESCALE));
                    Ok(Self {
                        writer,
                        input,
                        adaptor,
                    })
                }
            })
        }

        /// Write one frame at `ticks`, if the encoder is ready for it in time.
        pub(crate) fn append(
            &mut self,
            bgra8: &[u8],
            width: u32,
            height: u32,
            ticks: i64,
        ) -> Result<bool, VideoError> {
            autoreleasepool(|_| {
                // SAFETY: AVFoundation and CoreVideo used as documented on the
                // objects this writer holds; the pixel buffer is locked while
                // its memory is written, and every row written lies within it.
                unsafe {
                    let start = Instant::now();
                    while !self.input.isReadyForMoreMediaData() {
                        if start.elapsed() > READY_PATIENCE {
                            return Ok(false);
                        }
                        std::thread::sleep(std::time::Duration::from_micros(250));
                    }
                    let pool: Retained<CVPixelBufferPool> = self
                        .adaptor
                        .pixelBufferPool()
                        .ok_or_else(|| VideoError::Write(failure(&self.writer)))?;
                    let mut raw: *mut CVPixelBuffer = std::ptr::null_mut();
                    let made = CVPixelBufferPool::create_pixel_buffer(
                        None,
                        &pool,
                        NonNull::from(&mut raw),
                    );
                    let buffer: CFRetained<CVPixelBuffer> = match NonNull::new(raw) {
                        Some(raw) if made == kCVReturnSuccess => CFRetained::from_raw(raw),
                        _ => return Err(VideoError::Write(format!("no pixel buffer ({made})"))),
                    };
                    if CVPixelBufferLockBaseAddress(&buffer, CVPixelBufferLockFlags::empty())
                        != kCVReturnSuccess
                    {
                        return Err(VideoError::Write("could not lock a pixel buffer".into()));
                    }
                    let base = CVPixelBufferGetBaseAddress(&buffer).cast::<u8>();
                    let stride = CVPixelBufferGetBytesPerRow(&buffer);
                    let row = width as usize * 4;
                    if base.is_null() || stride < row {
                        CVPixelBufferUnlockBaseAddress(&buffer, CVPixelBufferLockFlags::empty());
                        return Err(VideoError::Write("a pixel buffer too small".into()));
                    }
                    for (y, pixels) in bgra8.chunks_exact(row).take(height as usize).enumerate() {
                        std::ptr::copy_nonoverlapping(pixels.as_ptr(), base.add(y * stride), row);
                    }
                    CVPixelBufferUnlockBaseAddress(&buffer, CVPixelBufferLockFlags::empty());
                    let time = CMTime::new(ticks, TIMESCALE);
                    if !self
                        .adaptor
                        .appendPixelBuffer_withPresentationTime(&buffer, time)
                    {
                        return Err(VideoError::Write(failure(&self.writer)));
                    }
                    Ok(true)
                }
            })
        }

        /// Finish the movie and wait for the file to be closed.
        pub(crate) fn finish(self) -> Result<(), VideoError> {
            autoreleasepool(|_| {
                // SAFETY: AVFoundation used as documented; the completion block
                // only sends on a channel, which is safe from any thread.
                unsafe {
                    self.input.markAsFinished();
                    let (done, finished) = mpsc::channel::<()>();
                    let block = RcBlock::new(move || {
                        let _ = done.send(());
                    });
                    self.writer.finishWritingWithCompletionHandler(&block);
                    finished.recv_timeout(FINISH_PATIENCE).map_err(|_| {
                        VideoError::Finish("the encoder did not finish in time".into())
                    })?;
                    if self.writer.status() == AVAssetWriterStatus::Completed {
                        Ok(())
                    } else {
                        Err(VideoError::Finish(failure(&self.writer)))
                    }
                }
            })
        }
    }

    /// Why the writer failed, as AVFoundation tells it.
    fn failure(writer: &AVAssetWriter) -> String {
        // SAFETY: reading the writer's documented `error` property.
        unsafe { writer.error() }.map_or_else(
            || "the encoder failed".to_string(),
            |e| e.localizedDescription().to_string(),
        )
    }
}

#[cfg(not(target_vendor = "apple"))]
mod platform {
    //! No encoder off Apple platforms.

    use std::path::Path;

    use super::VideoError;

    pub(crate) struct Writer;

    impl Writer {
        pub(crate) fn create(_: &Path, _: u32, _: u32, _: u32) -> Result<Self, VideoError> {
            Err(VideoError::Unsupported)
        }

        pub(crate) fn append(
            &mut self,
            _: &[u8],
            _: u32,
            _: u32,
            _: i64,
        ) -> Result<bool, VideoError> {
            Err(VideoError::Unsupported)
        }

        pub(crate) fn finish(self) -> Result<(), VideoError> {
            Err(VideoError::Unsupported)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_size_an_encoder_cannot_take_is_refused() {
        for (width, height) in [(321, 240), (320, 241), (8, 8), (0, 240)] {
            assert_eq!(
                VideoWriter::create("unused.mp4", width, height).err(),
                Some(VideoError::BadSize { width, height })
            );
        }
    }

    #[test]
    fn the_bit_rate_suits_the_size() {
        assert_eq!(bit_rate(1280, 720), 5_529_600);
        assert_eq!(bit_rate(64, 64), 1_000_000);
    }

    /// The `n`th frame of a test movie: a gradient sliding across it.
    #[cfg(target_vendor = "apple")]
    fn frame(n: u32, width: u32, height: u32) -> Vec<u8> {
        let mut pixels = Vec::with_capacity((width * height * 4) as usize);
        for y in 0..height {
            for x in 0..width {
                let r = ((x + 4 * n) % 256) as u8;
                pixels.extend_from_slice(&[(y % 256) as u8, 128, r, 255]);
            }
        }
        pixels
    }

    /// The sample count of the first `stsz` box in an MPEG-4 file: how many
    /// frames its video track holds.
    #[cfg(target_vendor = "apple")]
    fn samples(movie: &[u8]) -> Option<u32> {
        let at = movie.windows(4).position(|w| w == b"stsz")?;
        let count = movie.get(at + 12..at + 16)?;
        Some(u32::from_be_bytes(count.try_into().ok()?))
    }

    #[cfg(target_vendor = "apple")]
    #[test]
    fn frames_written_are_a_playable_h264_movie() {
        let dir = std::env::temp_dir().join(format!("stickshift-video-{}", std::process::id()));
        let path = dir.join("nested").join("movie.mp4");
        let (width, height) = (320, 240);
        let mut writer = VideoWriter::create(&path, width, height).expect("a movie");
        // The encoder works at the machine's pace: a frame it is not ready for
        // in time is dropped, so hand each over again until it is taken (a CI
        // runner's encoder, with no media engine, can be far slower than this
        // loop).
        let mut retried = 0;
        for n in 0..45 {
            let (pixels, at) = (frame(n, width, height), f64::from(n) / 30.0);
            let given_up = std::time::Instant::now() + Duration::from_secs(10);
            while !writer.append(&pixels, at).expect("a frame") {
                retried += 1;
                assert!(
                    std::time::Instant::now() < given_up,
                    "frame {n} never taken"
                );
                std::thread::sleep(Duration::from_millis(1));
            }
        }
        // Out of time order: dropped, not an error.
        assert_eq!(writer.append(&frame(0, width, height), 0.5), Ok(false));
        assert!(matches!(
            writer.append(&[0; 16], 2.0),
            Err(VideoError::BadFrame { .. })
        ));
        assert_eq!(writer.frames(), 45);
        let summary = writer.finish().expect("finished");
        assert_eq!((summary.width, summary.height), (width, height));
        assert_eq!(summary.frames, 45);
        // Each try the encoder was not ready for, and the frame out of order.
        assert_eq!(summary.dropped, retried + 1);
        assert!(
            (summary.seconds - 44.0 / 30.0).abs() < 1e-3,
            "{}",
            summary.seconds
        );

        let movie = std::fs::read(&path).expect("the movie");
        // An MPEG-4 file (`ftyp` first), holding every frame written, in H.264.
        assert_eq!(&movie[4..8], b"ftyp");
        assert_eq!(samples(&movie), Some(45));
        assert!(movie.windows(4).any(|w| w == b"avc1"), "not H.264");
        // And a second movie at the same path replaces it.
        let writer = VideoWriter::create(&path, width, height).expect("again");
        writer.finish().expect("an empty movie finished");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
