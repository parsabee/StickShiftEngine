// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

//! **Diagnostics capture**: footage of a session and a log of every frame, for
//! chasing down what goes wrong in it.
//!
//! Start and stop one from the built-in Graphics menu (*Start Diagnostics
//! Capture*, ⌃⌘D), and mark the moment something goes wrong (*Mark Moment*,
//! ⌃⌘M). A host of its own starts one with
//! [`Session::start_diagnostics`](crate::Session::start_diagnostics) or the
//! menu's commands, and [`RunConfig::diagnostics`](crate::RunConfig::diagnostics)
//! can start one at launch.
//!
//! Each capture is a folder of its own under [`DiagnosticsConfig::dir`], named
//! for the game and the local time it began (`offramp-20261121-153012`). It
//! holds:
//!
//! - **`footage.mp4`**: what the window showed, HUD and all, as a
//!   [`Recording`](crate::Recording) takes it, [`DiagnosticsConfig::footage`]
//!   pixels high. None while a recording of the whole run is under way (it
//!   holds the frames already), nor with `footage` off.
//! - **`frames.csv`**: a row for every frame shown, in the columns
//!   [`FRAME_COLUMNS`], then the game's own ([`Game::diagnostics`]):
//!   - its timing: the frame's period, the CPU's work in it, the GPU time of a
//!     frame that finished during it, physics and rendering, the updates run;
//!   - what the renderer drew and holds ([`FrameStats`]): draws, overlay
//!     quads, lights and lights left out of the light budget, live meshes and
//!     textures, GPU memory;
//!   - the process's resident memory, the scene's entities, rigid bodies and
//!     streamed entities;
//!   - the camera's world position and its distance from the origin, the
//!     rebases so far, and where the camera would be had the world never been
//!     rebased ([`Scene::rebase_total`](stickshift_scene::Scene::rebase_total)).
//!
//!   Its `time` is seconds since the capture began, on the clock the
//!   footage's frames carry, so a row is the footage at that time; `footage`
//!   says whether that frame went into the movie.
//! - **`events.csv`**: what happened when: the capture starting and stopping,
//!   each marked moment, rebases, hitches (a frame over [`HITCH`]), lights
//!   starting to be left out of the budget, settings changes, the view being
//!   resized or losing the keyboard.
//! - **`session.json`**: the machine (model, processor, memory, OS), the engine,
//!   the view and drawable, the graphics settings, the game's own facts
//!   ([`Game::diagnostics_info`]), and once the capture stops a summary: frame
//!   time percentiles and the worst frames, hitches, the furthest the camera got
//!   from the origin, peak memory, draws and lights, the rebases and the marked
//!   moments.
//!
//! [`Game::diagnostics`]: crate::Game::diagnostics
//! [`Game::diagnostics_info`]: crate::Game::diagnostics_info
//! [`FrameStats`]: stickshift_render_api::FrameStats

use std::fmt::{Display, Write as _};
use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use stickshift_math::glam::{DVec3, Vec3};
use stickshift_render_api::FrameStats;

use crate::context::Renderer;
use crate::record::{Recorder, Recording, VideoSummary};

/// A frame longer than this is a hitch: logged as an event, and counted.
pub const HITCH: Duration = Duration::from_millis(50);

/// The columns every row of a capture's `frames.csv` starts with, in order.
/// The game's own follow.
pub const FRAME_COLUMNS: [&str; 28] = [
    "frame",
    "time",
    "footage",
    "frame_ms",
    "cpu_ms",
    "gpu_ms",
    "physics_ms",
    "render_ms",
    "updates",
    "draws",
    "overlay_quads",
    "lights",
    "dropped_lights",
    "meshes",
    "textures",
    "gpu_mb",
    "memory_mb",
    "entities",
    "bodies",
    "streamed",
    "camera_x",
    "camera_y",
    "camera_z",
    "camera_distance",
    "rebases",
    "unshifted_x",
    "unshifted_y",
    "unshifted_z",
];

/// Where diagnostics captures go and what they hold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiagnosticsConfig {
    /// The folder each capture's own folder goes in; made if missing.
    /// Default `diagnostics`, under the working directory.
    pub dir: PathBuf,
    /// What each capture's folder is named for, before the time it began.
    /// Default: the executable's name.
    pub name: String,
    /// The footage's height in pixels (480 or 720, say: even, and at least
    /// 16), or `None` to take no footage. Default 720.
    pub footage: Option<u32>,
    /// Begin a capture as soon as the game's init has run. Default `false`.
    pub start_at_launch: bool,
}

impl Default for DiagnosticsConfig {
    fn default() -> Self {
        let name = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.file_stem().map(|s| s.to_string_lossy().into_owned()))
            .unwrap_or_else(|| String::from("capture"));
        Self {
            dir: PathBuf::from("diagnostics"),
            name,
            footage: Some(720),
            start_at_launch: false,
        }
    }
}

/// The values a game adds to one frame of a diagnostics capture
/// ([`Game::diagnostics`](crate::Game::diagnostics)): columns of its own in
/// `frames.csv`, after the engine's.
///
/// The names set on a capture's first frame are its columns. A later frame
/// that leaves one unset leaves it empty, and a name a later frame adds is
/// not written.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct DiagnosticsSample {
    values: Vec<(&'static str, String)>,
}

impl DiagnosticsSample {
    /// Set the column `name` to `value` for this frame, replacing any value
    /// it was already given.
    pub fn set(&mut self, name: &'static str, value: impl Display) {
        let value = value.to_string();
        match self.values.iter_mut().find(|(n, _)| *n == name) {
            Some((_, v)) => *v = value,
            None => self.values.push((name, value)),
        }
    }

    /// The value set for `name`, if one was.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&str> {
        self.values
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, v)| v.as_str())
    }

    fn clear(&mut self) {
        self.values.clear();
    }
}

/// The facts a game adds to a diagnostics capture's `session.json`
/// ([`Game::diagnostics_info`](crate::Game::diagnostics_info)): its version,
/// its seed, the flags it was launched with.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct DiagnosticsInfo {
    values: Vec<(String, String)>,
}

impl DiagnosticsInfo {
    /// Set the fact `name` to `value`, replacing any value it had.
    pub fn set(&mut self, name: impl Into<String>, value: impl Display) {
        let (name, value) = (name.into(), value.to_string());
        match self.values.iter_mut().find(|(n, _)| *n == name) {
            Some((_, v)) => *v = value,
            None => self.values.push((name, value)),
        }
    }

    /// The facts, in the order they were first set.
    #[must_use]
    pub fn values(&self) -> &[(String, String)] {
        &self.values
    }
}

/// What a finished diagnostics capture left behind
/// ([`Session::stop_diagnostics`](crate::Session::stop_diagnostics)).
#[derive(Debug)]
pub struct CaptureSummary {
    /// Its folder.
    pub folder: PathBuf,
    /// The frames it logged.
    pub frames: u64,
    /// How long it ran, in seconds of the frames it logged.
    pub seconds: f64,
    /// The moments marked in it.
    pub marks: usize,
    /// Its footage: the movie, or why there is none.
    pub footage: Result<VideoSummary, String>,
}

/// One frame's measurements, as the session gathers them for a capture.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct FrameReadings {
    /// The frame's period.
    pub frame: Duration,
    /// The CPU's work in it.
    pub cpu: Duration,
    /// The GPU time of a frame that finished during it, if one did.
    pub gpu: Option<Duration>,
    /// Physics and rendering.
    pub physics: Duration,
    pub render: Duration,
    /// Fixed updates run.
    pub updates: u32,
    /// What the renderer drew and holds, if it says.
    pub stats: Option<FrameStats>,
    /// The scene's entities, rigid bodies and streamed entities.
    pub entities: u32,
    pub bodies: u32,
    pub streamed: u32,
    /// The camera's world position.
    pub camera: Vec3,
    /// The rebases so far, and the sum of their offsets.
    pub rebases: u64,
    pub rebase_total: DVec3,
}

/// The view and settings a capture began with, for its `session.json`.
#[derive(Debug, Clone, Default)]
pub(crate) struct Setting {
    /// The view, in pixels and its pixels per point.
    pub view: Option<(u32, u32, f64)>,
    /// The drawable, in pixels.
    pub drawable: (u32, u32),
    /// The graphics settings, as `name: value` pairs.
    pub graphics: Vec<(&'static str, String)>,
}

/// A capture in progress.
pub(crate) struct Capture {
    folder: PathBuf,
    frames_csv: BufWriter<File>,
    events_csv: BufWriter<File>,
    /// The footage's recording, if it takes footage.
    footage: Option<Recorder>,
    /// Why there is no footage, when there is none.
    no_footage: Option<String>,
    /// Whether the footage asked for this frame.
    footage_due: bool,
    /// The game's columns, from the first frame.
    game_columns: Option<Vec<&'static str>>,
    sample: DiagnosticsSample,
    /// The facts for `session.json`.
    info: DiagnosticsInfo,
    setting: Setting,
    started: SystemTime,
    /// Seconds since it began: the footage's clock.
    clock: f64,
    frame: u64,
    /// Every frame's period, in milliseconds, for the percentiles.
    frame_ms: Vec<f32>,
    /// The worst frames: (milliseconds, frame, time).
    worst: Vec<(f32, u64, f64)>,
    hitches: u64,
    marks: Vec<(f64, u64)>,
    rebases_seen: Option<u64>,
    rebases: u64,
    dropping: bool,
    dropped_total: u64,
    peak: Peaks,
}

/// The most of each, over a capture.
#[derive(Debug, Default, Clone, Copy)]
struct Peaks {
    camera_distance: f32,
    camera_x: f32,
    camera_z: f32,
    memory: u64,
    gpu_memory: u64,
    draws: u32,
    lights: u32,
    meshes: u32,
    textures: u32,
    entities: u32,
}

impl Capture {
    /// Begin a capture as `config` says, its folder named for `started`
    /// (taken now if `None`), with the game's facts `info`, in `setting`.
    /// With `recording` (a recording of the whole run under way), it takes no
    /// footage of its own.
    pub(crate) fn start(
        config: &DiagnosticsConfig,
        info: DiagnosticsInfo,
        setting: Setting,
        recording: bool,
    ) -> io::Result<Self> {
        let started = SystemTime::now();
        let folder = unique_folder(&config.dir, &config.name, started)?;
        let mut frames_csv = BufWriter::new(File::create(folder.join("frames.csv"))?);
        let mut events_csv = BufWriter::new(File::create(folder.join("events.csv"))?);
        writeln!(events_csv, "time,frame,kind,detail")?;
        let (footage, no_footage) = match (config.footage, recording) {
            (_, true) => (
                None,
                Some(String::from(
                    "a recording of the whole run is under way: its movie has the frames",
                )),
            ),
            (None, false) => (None, Some(String::from("footage was turned off"))),
            (Some(height), false) => (
                Some(Recorder::new(Recording::new(
                    folder.join("footage.mp4"),
                    height,
                ))),
                None,
            ),
        };
        // The header waits for the first frame, which names the game's columns.
        frames_csv.flush()?;
        let mut capture = Self {
            folder,
            frames_csv,
            events_csv,
            footage,
            no_footage,
            footage_due: false,
            game_columns: None,
            sample: DiagnosticsSample::default(),
            info,
            setting,
            started,
            clock: 0.0,
            frame: 0,
            frame_ms: Vec::new(),
            worst: Vec::new(),
            hitches: 0,
            marks: Vec::new(),
            rebases_seen: None,
            rebases: 0,
            dropping: false,
            dropped_total: 0,
            peak: Peaks::default(),
        };
        capture.event("start", "")?;
        capture.write_session(None)?;
        Ok(capture)
    }

    /// Its folder.
    pub(crate) fn folder(&self) -> &Path {
        &self.folder
    }

    /// Before a frame shown `elapsed` after the last, in a view of `aspect`:
    /// the footage takes it if it is due.
    pub(crate) fn before_frame(
        &mut self,
        renderer: &mut dyn Renderer,
        elapsed: Duration,
        aspect: f32,
    ) {
        self.clock += elapsed.as_secs_f64();
        self.footage_due = self
            .footage
            .as_mut()
            .is_some_and(|footage| footage.before_frame(renderer, elapsed, aspect));
    }

    /// After a frame: the footage writes what the GPU has finished.
    pub(crate) fn after_frame(&mut self, renderer: &mut dyn Renderer) {
        if let Some(footage) = &mut self.footage {
            footage.after_frame(renderer);
        }
    }

    /// The sample for the game to fill this frame, emptied.
    pub(crate) fn sample(&mut self) -> &mut DiagnosticsSample {
        self.sample.clear();
        &mut self.sample
    }

    /// Log this frame's `readings`, and the game's sample.
    pub(crate) fn log_frame(&mut self, readings: &FrameReadings) -> io::Result<()> {
        if self.game_columns.is_none() {
            let columns: Vec<&'static str> =
                self.sample.values.iter().map(|(name, _)| *name).collect();
            let header: Vec<&str> = FRAME_COLUMNS
                .iter()
                .copied()
                .chain(columns.iter().copied())
                .collect();
            writeln!(self.frames_csv, "{}", header.join(","))?;
            self.game_columns = Some(columns);
        }
        let ms = |d: Duration| d.as_secs_f64() * 1000.0;
        let stats = readings.stats.unwrap_or_default();
        let memory = resident_memory();
        let camera = readings.camera;
        let distance = camera.length();
        let unshifted = camera.as_dvec3() - readings.rebase_total;
        let mut row = String::with_capacity(256);
        let _ = write!(
            row,
            "{},{:.4},{},{:.3},{:.3},",
            self.frame,
            self.clock,
            u8::from(self.footage_due),
            ms(readings.frame),
            ms(readings.cpu),
        );
        if let Some(gpu) = readings.gpu {
            let _ = write!(row, "{:.3}", ms(gpu));
        }
        let _ = write!(
            row,
            ",{:.3},{:.3},{},{},{},{},{},{},{},{:.1},{},{},{},{},{:.3},{:.3},{:.3},{:.3},{},{:.3},{:.3},{:.3}",
            ms(readings.physics),
            ms(readings.render),
            readings.updates,
            stats.draws,
            stats.overlay_quads,
            stats.lights,
            stats.dropped_lights,
            stats.meshes,
            stats.textures,
            stats.gpu_memory as f64 / 1_048_576.0,
            memory.map_or(String::new(), |m| format!("{:.1}", m as f64 / 1_048_576.0)),
            readings.entities,
            readings.bodies,
            readings.streamed,
            camera.x,
            camera.y,
            camera.z,
            distance,
            readings.rebases,
            unshifted.x,
            unshifted.y,
            unshifted.z,
        );
        for name in self.game_columns.as_deref().unwrap_or_default() {
            row.push(',');
            if let Some(value) = self.sample.get(name) {
                row.push_str(&csv_field(value));
            }
        }
        writeln!(self.frames_csv, "{row}")?;

        // The events this frame makes.
        let frame_ms = ms(readings.frame) as f32;
        if readings.frame > HITCH {
            self.hitches += 1;
            self.event("hitch", format!("{frame_ms:.1} ms"))?;
        }
        if let Some(seen) = self.rebases_seen {
            if readings.rebases > seen {
                self.rebases += readings.rebases - seen;
                self.event(
                    "rebase",
                    format!(
                        "camera now at ({:.1} {:.1} {:.1}); shifted {:.0} {:.0} {:.0} in all",
                        camera.x,
                        camera.y,
                        camera.z,
                        readings.rebase_total.x,
                        readings.rebase_total.y,
                        readings.rebase_total.z
                    ),
                )?;
            }
        }
        self.rebases_seen = Some(readings.rebases);
        if stats.dropped_lights > 0 && !self.dropping {
            self.event(
                "dropped_lights",
                format!(
                    "{} of {} lights left out",
                    stats.dropped_lights, stats.lights
                ),
            )?;
        }
        self.dropping = stats.dropped_lights > 0;
        self.dropped_total += u64::from(stats.dropped_lights);

        // The summary's tallies.
        self.frame_ms.push(frame_ms);
        self.worst.push((frame_ms, self.frame, self.clock));
        self.worst.sort_by(|a, b| b.0.total_cmp(&a.0));
        self.worst.truncate(10);
        let peak = &mut self.peak;
        peak.camera_distance = peak.camera_distance.max(distance);
        peak.camera_x = peak.camera_x.max(camera.x.abs());
        peak.camera_z = peak.camera_z.max(camera.z.abs());
        peak.memory = peak.memory.max(memory.unwrap_or(0));
        peak.gpu_memory = peak.gpu_memory.max(stats.gpu_memory);
        peak.draws = peak.draws.max(stats.draws);
        peak.lights = peak.lights.max(stats.lights);
        peak.meshes = peak.meshes.max(stats.meshes);
        peak.textures = peak.textures.max(stats.textures);
        peak.entities = peak.entities.max(readings.entities);
        self.frame += 1;
        Ok(())
    }

    /// Log an event of `kind`, now.
    pub(crate) fn event(&mut self, kind: &str, detail: impl Display) -> io::Result<()> {
        writeln!(
            self.events_csv,
            "{:.4},{},{},{}",
            self.clock,
            self.frame,
            kind,
            csv_field(&detail.to_string())
        )?;
        self.events_csv.flush()
    }

    /// Mark this moment.
    pub(crate) fn mark(&mut self) -> io::Result<()> {
        self.marks.push((self.clock, self.frame));
        let n = self.marks.len();
        self.event("mark", format!("moment {n}"))
    }

    /// Stop: finish the footage, and write the summary.
    pub(crate) fn finish(mut self, renderer: &mut dyn Renderer) -> io::Result<CaptureSummary> {
        let footage = match self.footage.take() {
            Some(footage) => footage.finish(renderer).map_err(|e| e.to_string()),
            None => Err(self
                .no_footage
                .clone()
                .unwrap_or_else(|| String::from("none taken"))),
        };
        self.event("stop", "")?;
        self.frames_csv.flush()?;
        self.write_session(Some(&footage))?;
        Ok(CaptureSummary {
            folder: self.folder.clone(),
            frames: self.frame,
            seconds: self.clock,
            marks: self.marks.len(),
            footage,
        })
    }

    /// Write `session.json`: what it began with, and with `footage` (once it
    /// has stopped) the summary.
    fn write_session(&self, footage: Option<&Result<VideoSummary, String>>) -> io::Result<()> {
        let mut json = Json::default();
        json.open();
        json.key("started");
        json.string(&local_time(self.started, "%Y-%m-%dT%H:%M:%S"));
        json.key("engine");
        json.open();
        json.pair("name", "Stick Shift Engine");
        json.pair("version", env!("CARGO_PKG_VERSION"));
        json.close();
        json.key("machine");
        json.open();
        for (name, key) in [
            ("model", "hw.model"),
            ("processor", "machdep.cpu.brand_string"),
            ("os", "kern.osproductversion"),
            ("os_build", "kern.osversion"),
        ] {
            json.pair(name, sysctl_string(key).as_deref().unwrap_or("unknown"));
        }
        json.key("memory_gb");
        json.number(sysctl_u64("hw.memsize").map_or(0.0, |b| b as f64 / 1_073_741_824.0));
        json.close();
        json.key("view");
        json.open();
        if let Some((w, h, scale)) = self.setting.view {
            json.key("width");
            json.number(f64::from(w));
            json.key("height");
            json.number(f64::from(h));
            json.key("scale");
            json.number(scale);
        }
        json.key("drawable_width");
        json.number(f64::from(self.setting.drawable.0));
        json.key("drawable_height");
        json.number(f64::from(self.setting.drawable.1));
        json.close();
        json.key("graphics");
        json.open();
        for (name, value) in &self.setting.graphics {
            json.pair(name, value);
        }
        json.close();
        json.key("game");
        json.open();
        for (name, value) in self.info.values() {
            json.pair(name, value);
        }
        json.close();
        json.key("footage");
        match footage {
            None if self.footage.is_some() => json.string("footage.mp4 (being taken)"),
            None => json.string(self.no_footage.as_deref().unwrap_or("none")),
            Some(Ok(movie)) => {
                json.open();
                json.pair("file", "footage.mp4");
                json.key("width");
                json.number(f64::from(movie.width));
                json.key("height");
                json.number(f64::from(movie.height));
                json.key("frames");
                json.number(movie.frames as f64);
                json.key("dropped");
                json.number(movie.dropped as f64);
                json.close();
            }
            Some(Err(why)) => json.string(why),
        }
        if footage.is_some() {
            self.write_summary(&mut json);
        }
        json.close();
        std::fs::write(self.folder.join("session.json"), json.finish())
    }

    /// The summary, into `json`.
    fn write_summary(&self, json: &mut Json) {
        let mut sorted = self.frame_ms.clone();
        sorted.sort_by(f32::total_cmp);
        let percentile = |p: f64| -> f64 {
            if sorted.is_empty() {
                return 0.0;
            }
            let i = ((sorted.len() - 1) as f64 * p).round() as usize;
            f64::from(sorted[i])
        };
        json.key("summary");
        json.open();
        json.key("frames");
        json.number(self.frame as f64);
        json.key("seconds");
        json.number(self.clock);
        json.key("average_fps");
        json.number(if self.clock > 0.0 {
            self.frame as f64 / self.clock
        } else {
            0.0
        });
        json.key("frame_ms");
        json.open();
        for (name, p) in [("p50", 0.5), ("p95", 0.95), ("p99", 0.99), ("max", 1.0)] {
            json.key(name);
            json.number(percentile(p));
        }
        json.close();
        json.key("hitches");
        json.number(self.hitches as f64);
        json.key("worst_frames");
        json.array_open();
        for (ms, frame, time) in &self.worst {
            json.open();
            json.key("frame");
            json.number(*frame as f64);
            json.key("time");
            json.number(*time);
            json.key("ms");
            json.number(f64::from(*ms));
            json.close();
        }
        json.array_close();
        let peak = self.peak;
        json.key("furthest_camera_from_origin");
        json.number(f64::from(peak.camera_distance));
        json.key("furthest_camera_x");
        json.number(f64::from(peak.camera_x));
        json.key("furthest_camera_z");
        json.number(f64::from(peak.camera_z));
        json.key("rebases");
        json.number(self.rebases as f64);
        json.key("peak_memory_mb");
        json.number(peak.memory as f64 / 1_048_576.0);
        json.key("peak_gpu_memory_mb");
        json.number(peak.gpu_memory as f64 / 1_048_576.0);
        json.key("peak_draws");
        json.number(f64::from(peak.draws));
        json.key("peak_lights");
        json.number(f64::from(peak.lights));
        json.key("lights_left_out");
        json.number(self.dropped_total as f64);
        json.key("peak_meshes");
        json.number(f64::from(peak.meshes));
        json.key("peak_textures");
        json.number(f64::from(peak.textures));
        json.key("peak_entities");
        json.number(f64::from(peak.entities));
        json.key("marks");
        json.array_open();
        for (time, frame) in &self.marks {
            json.open();
            json.key("frame");
            json.number(*frame as f64);
            json.key("time");
            json.number(*time);
            json.close();
        }
        json.array_close();
        json.close();
    }
}

/// A folder of its own under `dir` for a capture named `name`, begun at
/// `started`: `name-YYYYMMDD-HHMMSS`, with `-2`, `-3`… if that is taken.
fn unique_folder(dir: &Path, name: &str, started: SystemTime) -> io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let stem = format!("{name}-{}", local_time(started, "%Y%m%d-%H%M%S"));
    for n in 1u32.. {
        let folder = if n == 1 {
            dir.join(&stem)
        } else {
            dir.join(format!("{stem}-{n}"))
        };
        match std::fs::create_dir(&folder) {
            Ok(()) => return Ok(folder),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    unreachable!("some folder name is free")
}

/// `value` as a CSV field: quoted, its quotes doubled, if it holds a comma,
/// a quote or a line break.
fn csv_field(value: &str) -> String {
    if value.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_owned()
    }
}

/// A JSON document, written as it goes: just enough for `session.json`.
#[derive(Default)]
struct Json {
    out: String,
    /// Whether the open object or array has had an entry, a level each.
    entries: Vec<bool>,
    /// Whether a key was just written, so its value takes no comma.
    after_key: bool,
}

impl Json {
    fn separate(&mut self) {
        if self.after_key {
            self.after_key = false;
            return;
        }
        if let Some(had) = self.entries.last_mut() {
            if *had {
                self.out.push(',');
            }
            *had = true;
            self.out.push('\n');
            self.out.push_str(&"  ".repeat(self.entries.len()));
        }
    }
    fn open(&mut self) {
        self.separate();
        self.out.push('{');
        self.entries.push(false);
    }
    fn close(&mut self) {
        let had = self.entries.pop().unwrap_or(false);
        if had {
            self.out.push('\n');
            self.out.push_str(&"  ".repeat(self.entries.len()));
        }
        self.out.push('}');
    }
    fn array_open(&mut self) {
        self.separate();
        self.out.push('[');
        self.entries.push(false);
    }
    fn array_close(&mut self) {
        let had = self.entries.pop().unwrap_or(false);
        if had {
            self.out.push('\n');
            self.out.push_str(&"  ".repeat(self.entries.len()));
        }
        self.out.push(']');
    }
    fn key(&mut self, key: &str) {
        self.separate();
        self.push_string(key);
        self.out.push_str(": ");
        self.after_key = true;
    }
    fn string(&mut self, value: &str) {
        self.separate();
        self.push_string(value);
    }
    fn number(&mut self, value: f64) {
        self.separate();
        if value.is_finite() {
            if value.fract() == 0.0 && value.abs() < 1e15 {
                let _ = write!(self.out, "{}", value as i64);
            } else {
                let _ = write!(self.out, "{value:.4}");
            }
        } else {
            self.out.push_str("null");
        }
    }
    fn pair(&mut self, key: &str, value: &str) {
        self.key(key);
        self.string(value);
    }
    fn push_string(&mut self, value: &str) {
        self.out.push('"');
        for c in value.chars() {
            match c {
                '"' => self.out.push_str("\\\""),
                '\\' => self.out.push_str("\\\\"),
                '\n' => self.out.push_str("\\n"),
                '\r' => self.out.push_str("\\r"),
                '\t' => self.out.push_str("\\t"),
                c if (c as u32) < 0x20 => {
                    let _ = write!(self.out, "\\u{:04x}", c as u32);
                }
                c => self.out.push(c),
            }
        }
        self.out.push('"');
    }
    fn finish(mut self) -> String {
        self.out.push('\n');
        self.out
    }
}

/// `time` in the local time zone, formatted with `%Y %m %d %H %M %S` (the
/// only fields it knows).
fn local_time(time: SystemTime, format: &str) -> String {
    let fields = local_fields(time);
    let mut out = String::with_capacity(format.len() + 8);
    let mut chars = format.chars();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('Y') => {
                let _ = write!(out, "{:04}", fields[0]);
            }
            Some('m') => {
                let _ = write!(out, "{:02}", fields[1]);
            }
            Some('d') => {
                let _ = write!(out, "{:02}", fields[2]);
            }
            Some('H') => {
                let _ = write!(out, "{:02}", fields[3]);
            }
            Some('M') => {
                let _ = write!(out, "{:02}", fields[4]);
            }
            Some('S') => {
                let _ = write!(out, "{:02}", fields[5]);
            }
            Some(other) => {
                out.push('%');
                out.push(other);
            }
            None => out.push('%'),
        }
    }
    out
}

/// `time`'s year, month, day, hour, minute and second, in the local time
/// zone (UTC where it cannot be had).
fn local_fields(time: SystemTime) -> [i64; 6] {
    let seconds = time
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    #[cfg(unix)]
    {
        let t = seconds as libc::time_t;
        let mut tm = std::mem::MaybeUninit::<libc::tm>::zeroed();
        // SAFETY: `localtime_r` writes a `tm` into the buffer it is given
        // and reads only `t`; both live for the call.
        let ok = unsafe { !libc::localtime_r(&t, tm.as_mut_ptr()).is_null() };
        if ok {
            // SAFETY: `localtime_r` succeeded, so it filled `tm` in.
            let tm = unsafe { tm.assume_init() };
            return [
                i64::from(tm.tm_year) + 1900,
                i64::from(tm.tm_mon) + 1,
                i64::from(tm.tm_mday),
                i64::from(tm.tm_hour),
                i64::from(tm.tm_min),
                i64::from(tm.tm_sec),
            ];
        }
    }
    utc_fields(seconds)
}

/// The UTC date and time `seconds` after the Unix epoch: year, month, day,
/// hour, minute, second.
fn utc_fields(seconds: i64) -> [i64; 6] {
    let (days, rest) = (seconds.div_euclid(86_400), seconds.rem_euclid(86_400));
    // Howard Hinnant's days-to-civil.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    [year, month, day, rest / 3600, rest % 3600 / 60, rest % 60]
}

/// The process's resident memory in bytes, where the platform says.
fn resident_memory() -> Option<u64> {
    #[cfg(target_vendor = "apple")]
    {
        let mut info = std::mem::MaybeUninit::<libc::mach_task_basic_info>::zeroed();
        let mut count = libc::MACH_TASK_BASIC_INFO_COUNT;
        // `mach_task_self` is deprecated in `libc` in favour of the `mach2`
        // crate, a new dependency for this one call.
        #[allow(deprecated)]
        // SAFETY: `task_info` writes at most `count` words of the flavour's
        // struct into the buffer, which is exactly that big, and updates
        // `count`; the port is this task's own.
        let result = unsafe {
            libc::task_info(
                libc::mach_task_self(),
                libc::MACH_TASK_BASIC_INFO,
                info.as_mut_ptr().cast(),
                &mut count,
            )
        };
        if result == libc::KERN_SUCCESS {
            // SAFETY: `task_info` succeeded, so it filled `info` in.
            return Some(unsafe { info.assume_init() }.resident_size);
        }
    }
    None
}

/// A string `sysctl` value, by name.
fn sysctl_string(name: &str) -> Option<String> {
    #[cfg(target_vendor = "apple")]
    {
        let name = std::ffi::CString::new(name).ok()?;
        let mut len = 0usize;
        // SAFETY: a null buffer asks only for the value's size, into `len`.
        let sized = unsafe {
            libc::sysctlbyname(
                name.as_ptr(),
                std::ptr::null_mut(),
                &mut len,
                std::ptr::null_mut(),
                0,
            )
        };
        if sized != 0 || len == 0 {
            return None;
        }
        let mut buffer = vec![0u8; len];
        // SAFETY: the buffer is `len` bytes, as `sysctlbyname` was told.
        let read = unsafe {
            libc::sysctlbyname(
                name.as_ptr(),
                buffer.as_mut_ptr().cast(),
                &mut len,
                std::ptr::null_mut(),
                0,
            )
        };
        if read != 0 {
            return None;
        }
        buffer.truncate(len);
        while buffer.last() == Some(&0) {
            buffer.pop();
        }
        return String::from_utf8(buffer).ok();
    }
    #[allow(unreachable_code)]
    {
        let _ = name;
        None
    }
}

/// A 64-bit integer `sysctl` value, by name.
fn sysctl_u64(name: &str) -> Option<u64> {
    #[cfg(target_vendor = "apple")]
    {
        let name = std::ffi::CString::new(name).ok()?;
        let mut value = 0u64;
        let mut len = std::mem::size_of::<u64>();
        // SAFETY: the buffer is a `u64`, `len` bytes, as `sysctlbyname` is told.
        let read = unsafe {
            libc::sysctlbyname(
                name.as_ptr(),
                std::ptr::addr_of_mut!(value).cast(),
                &mut len,
                std::ptr::null_mut(),
                0,
            )
        };
        return (read == 0).then_some(value);
    }
    #[allow(unreachable_code)]
    {
        let _ = name;
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::context::EngineCtx;
    use crate::driver::Loop;
    use crate::game::Game;
    use crate::record::Recording;
    use crate::session::{Clock, HostEvent, Requests, Session, View};
    use crate::settings_menu::MenuCommand;
    use stickshift_render_api::NullRenderer;

    /// A game that adds columns and facts of its own, and rebases once.
    #[derive(Default)]
    struct Probe {
        updates: u32,
        rebase_at: Option<u32>,
    }

    impl Game for Probe {
        fn init(&mut self, _ctx: &mut EngineCtx) {}

        fn update(&mut self, ctx: &mut EngineCtx, _dt: f32) {
            self.updates += 1;
            if Some(self.updates) == self.rebase_at {
                let _ = ctx
                    .scene_mut()
                    .maybe_rebase(Vec3::new(2_500.0, 0.0, -100.0));
            }
        }

        fn render(&mut self, ctx: &mut EngineCtx) {
            let renderer = ctx.renderer();
            renderer.begin_frame();
            renderer.submit();
        }

        fn diagnostics(&self, sample: &mut DiagnosticsSample) {
            sample.set("updates_so_far", self.updates);
            sample.set("note", "a, b");
        }

        fn diagnostics_info(&self, info: &mut DiagnosticsInfo) {
            info.set("seed", 42);
        }
    }

    /// A scratch folder for one test.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ssd-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    /// `line`'s fields, quoted ones whole.
    fn fields(line: &str) -> Vec<String> {
        let (mut out, mut field, mut quoted) = (Vec::new(), String::new(), false);
        let mut chars = line.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '"' if quoted && chars.peek() == Some(&'"') => {
                    field.push('"');
                    chars.next();
                }
                '"' => quoted = !quoted,
                ',' if !quoted => out.push(std::mem::take(&mut field)),
                c => field.push(c),
            }
        }
        out.push(field);
        out
    }

    #[test]
    fn a_capture_from_the_menu_logs_every_frame_its_events_and_a_summary_beside_its_footage() {
        let dir = scratch("menu");
        let mut session = Session::new(Loop::new(), None, true);
        session.set_diagnostics(DiagnosticsConfig {
            dir: dir.clone(),
            name: String::from("probe"),
            footage: Some(240),
            start_at_launch: false,
        });
        let (mut game, mut renderer) = (Probe::default(), NullRenderer::new());
        let _ = session.resume(View::new(1280, 720, 1.0), &mut game, &mut renderer);
        let choose = |session: &mut Session, game: &mut Probe, renderer: &mut NullRenderer, c| {
            session.event(HostEvent::Menu(c), game, renderer)
        };
        // Nothing to mark yet.
        assert!(!session.menu_state(false).capturing);
        assert_eq!(
            choose(
                &mut session,
                &mut game,
                &mut renderer,
                MenuCommand::MarkMoment
            ),
            Requests::NONE
        );
        // Start it from the menu.
        let started = choose(
            &mut session,
            &mut game,
            &mut renderer,
            MenuCommand::ToggleDiagnostics,
        );
        assert!(started.contains(Requests::MENU_SYNC));
        assert!(session.menu_state(false).capturing);
        let folder = session.capture_folder().expect("capturing").to_path_buf();
        assert!(folder.starts_with(&dir));
        game.rebase_at = Some(game.updates + 30);
        for frame in 0..90 {
            if frame == 45 {
                // A hitch, and a moment marked in it.
                std::thread::sleep(Duration::from_millis(60));
                let _ = choose(
                    &mut session,
                    &mut game,
                    &mut renderer,
                    MenuCommand::MarkMoment,
                );
            }
            let _ = session.frame(Clock::FixedStep, &mut game, &mut renderer);
        }
        let _ = session.event(
            HostEvent::Resized(View::new(1000, 700, 1.0)),
            &mut game,
            &mut renderer,
        );
        for _ in 0..10 {
            let _ = session.frame(Clock::FixedStep, &mut game, &mut renderer);
        }
        let stopped = choose(
            &mut session,
            &mut game,
            &mut renderer,
            MenuCommand::ToggleDiagnostics,
        );
        assert!(stopped.contains(Requests::MENU_SYNC));
        assert!(!session.menu_state(false).capturing);

        // A row a frame: the engine's columns, then the game's.
        let frames = std::fs::read_to_string(folder.join("frames.csv")).expect("frames.csv");
        let mut lines = frames.lines();
        let header = fields(lines.next().expect("a header"));
        let mut expected: Vec<String> = FRAME_COLUMNS.iter().map(|c| (*c).to_owned()).collect();
        expected.extend(["updates_so_far", "note"].map(String::from));
        assert_eq!(header, expected);
        let rows: Vec<Vec<String>> = lines.map(fields).collect();
        assert_eq!(rows.len(), 100);
        let column = |name: &str| header.iter().position(|c| c == name).expect(name);
        for (i, row) in rows.iter().enumerate() {
            assert_eq!(row.len(), header.len(), "row {i}");
            assert_eq!(row[column("frame")], i.to_string());
            assert_eq!(row[column("updates")], "1");
            assert_eq!(row[column("note")], "a, b");
        }
        let updates: Vec<u32> = rows
            .iter()
            .map(|row| row[column("updates_so_far")].parse().expect("a number"))
            .collect();
        assert!(
            updates.windows(2).all(|w| w[1] == w[0] + 1),
            "the game's values, a frame each"
        );
        assert!(rows.iter().any(|row| row[column("footage")] == "1"));
        let rebases: Vec<&str> = rows
            .iter()
            .map(|row| row[column("rebases")].as_str())
            .collect();
        assert_eq!(rebases.first(), Some(&"0"));
        assert_eq!(rebases.last(), Some(&"1"), "the rebase counted");
        let last = rows.last().expect("rows");
        let x: f64 = last[column("unshifted_x")].parse().expect("x");
        let shifted: f64 = last[column("camera_x")].parse().expect("x");
        assert!(
            (x - shifted - 2_000.0).abs() < 1e-3,
            "where the camera truly is"
        );
        assert!(last[column("draws")].parse::<u32>().is_ok());

        // The events, in order.
        let events = std::fs::read_to_string(folder.join("events.csv")).expect("events.csv");
        let kinds: Vec<String> = events
            .lines()
            .skip(1)
            .map(|l| fields(l)[2].clone())
            .collect();
        for kind in ["start", "rebase", "hitch", "mark", "resize", "stop"] {
            assert!(kinds.iter().any(|k| k == kind), "{kind} in {kinds:?}");
        }
        assert_eq!(kinds.first().map(String::as_str), Some("start"));
        assert_eq!(kinds.last().map(String::as_str), Some("stop"));

        // The session, and its summary.
        let json = std::fs::read_to_string(folder.join("session.json")).expect("session.json");
        for needle in [
            "\"seed\": \"42\"",
            "\"summary\"",
            "\"frames\": 100",
            "\"rebases\": 1",
            "\"file\": \"footage.mp4\"",
            "\"marks\": [",
            "\"processor\"",
            "\"draw_distance\"",
        ] {
            assert!(json.contains(needle), "{needle} in {json}");
        }
        let footage = std::fs::metadata(folder.join("footage.mp4")).expect("footage");
        assert!(footage.len() > 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_capture_asked_for_at_launch_starts_and_takes_no_footage_beside_a_recording() {
        let dir = scratch("launch");
        let mut session = Session::new(Loop::new(), None, true);
        session.set_diagnostics(DiagnosticsConfig {
            dir: dir.clone(),
            name: String::from("probe"),
            footage: Some(240),
            start_at_launch: true,
        });
        session.record(Recording::new(dir.join("run.mp4"), 240));
        let (mut game, mut renderer) = (Probe::default(), NullRenderer::new());
        let _ = session.resume(View::new(1280, 720, 1.0), &mut game, &mut renderer);
        let folder = session
            .capture_folder()
            .expect("started at launch")
            .to_path_buf();
        for _ in 0..20 {
            let _ = session.frame(Clock::FixedStep, &mut game, &mut renderer);
        }
        // A second resume (back from the background) starts no second one.
        let _ = session.resume(View::new(1280, 720, 1.0), &mut game, &mut renderer);
        assert_eq!(session.capture_folder(), Some(folder.as_path()));
        let summary = session
            .stop_diagnostics(&mut renderer)
            .expect("a capture")
            .expect("finished");
        assert_eq!(summary.frames, 20);
        assert!(summary.footage.is_err(), "no footage of its own");
        assert!(!folder.join("footage.mp4").exists());
        let json = std::fs::read_to_string(folder.join("session.json")).expect("session.json");
        assert!(json.contains("recording of the whole run"), "{json}");
        // The recording has the frames.
        let movie = session
            .finish_recording(&mut renderer)
            .expect("a recording");
        assert!(movie.is_ok_and(|m| m.frames > 0));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn csv_fields_with_commas_or_quotes_are_quoted() {
        assert_eq!(csv_field("plain"), "plain");
        assert_eq!(csv_field("a, b"), "\"a, b\"");
        assert_eq!(csv_field("say \"hi\""), "\"say \"\"hi\"\"\"");
    }

    #[test]
    fn json_nests_and_escapes() {
        let mut json = Json::default();
        json.open();
        json.pair("name", "a \"b\"\n");
        json.key("list");
        json.array_open();
        json.number(1.0);
        json.number(2.5);
        json.array_close();
        json.key("empty");
        json.open();
        json.close();
        json.close();
        assert_eq!(
            json.finish(),
            "{\n  \"name\": \"a \\\"b\\\"\\n\",\n  \"list\": [\n    1,\n    2.5000\n  ],\n  \"empty\": {}\n}\n"
        );
    }

    #[test]
    fn utc_dates_come_out_right() {
        assert_eq!(utc_fields(0), [1970, 1, 1, 0, 0, 0]);
        // 2000-02-29 12:34:56 UTC.
        assert_eq!(utc_fields(951_827_696), [2000, 2, 29, 12, 34, 56]);
        assert_eq!(utc_fields(-1), [1969, 12, 31, 23, 59, 59]);
    }

    #[test]
    fn a_sample_keeps_one_value_a_column() {
        let mut sample = DiagnosticsSample::default();
        sample.set("speed", 14.5);
        sample.set("state", "playing");
        sample.set("speed", 15);
        assert_eq!(sample.get("speed"), Some("15"));
        assert_eq!(sample.values.len(), 2);
        assert_eq!(sample.get("missing"), None);
    }

    #[test]
    fn the_machine_and_memory_can_be_read() {
        assert!(resident_memory().is_some_and(|m| m > 1_000_000));
        assert!(sysctl_string("hw.model").is_some_and(|m| !m.is_empty()));
        assert!(sysctl_u64("hw.memsize").is_some_and(|m| m > 1 << 30));
    }

    #[test]
    fn folders_are_named_for_the_time_and_never_reused() {
        let dir = std::env::temp_dir().join(format!("ssd-folders-{}", std::process::id()));
        let at = SystemTime::now();
        let first = unique_folder(&dir, "game", at).expect("a folder");
        let second = unique_folder(&dir, "game", at).expect("another");
        assert_ne!(first, second);
        let name = first.file_name().unwrap().to_string_lossy().into_owned();
        assert!(
            name.starts_with("game-") && name.len() == "game-20261121-153012".len(),
            "{name}"
        );
        assert!(second.to_string_lossy().ends_with("-2"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
