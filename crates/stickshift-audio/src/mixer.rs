// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

//! [`Audio`] — the whole audio surface, and its device-free silent mode.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use kira::sound::static_sound::{StaticSoundData, StaticSoundHandle};
use kira::{AudioManager, AudioManagerSettings, Decibels, DefaultBackend, PlaybackRate, Tween};

/// The id the next [`Audio`] takes, so its handles name it.
static NEXT_OWNER: AtomicU32 = AtomicU32::new(1);

use crate::handle::{LayerHandle, SoundHandle};
use crate::volume::Volume;

/// One loaded sound's bookkeeping.
struct Sound {
    /// The path it was loaded from — the cache key, kept so
    /// [`Audio::path`] can report it (and so a decode failure can name the file).
    path: PathBuf,
    /// The decoded samples, ready to hand to the mixer. `None` when nothing was
    /// decoded: either the whole [`Audio`] is silent (so decoding would be pure
    /// waste) or the file could not be read, which degrades this one sound to
    /// silence rather than failing the load.
    data: Option<StaticSoundData>,
    /// How many one-shots of this sound have been requested.
    one_shots: u32,
}

/// One playing layer's bookkeeping.
struct Layer {
    /// The sound it loops.
    sound: SoundHandle,
    /// The level it was last set to (or is fading to).
    volume: Volume,
    /// The playback rate it was last set to (or is gliding to): `1.0` as
    /// recorded, `2.0` twice as fast and an octave up.
    rate: f64,
    /// Its `kira` handle, on a device and when the sound decoded.
    playing: Option<StaticSoundHandle>,
}

/// A fade of `duration`, from now, along a straight line.
fn fade(duration: Duration) -> Tween {
    Tween {
        duration,
        ..Tween::default()
    }
}

/// Where playback actually goes.
enum Output {
    /// A live `kira` mixer holding an open output device.
    Device(Box<Device>),
    /// No device: every play is accepted and produces nothing.
    Silent,
}

/// The `kira` mixer behind a device. Tests drive the same device-mode code over
/// `kira`'s mock backend, which opens no device and produces no sound.
// The second variant exists only in tests, and a `Device` already lives in a
// `Box`, so the size difference costs nothing outside tests.
#[cfg_attr(test, allow(clippy::large_enum_variant))]
enum Manager {
    Default(AudioManager<DefaultBackend>),
    #[cfg(test)]
    Mock(AudioManager<kira::backend::mock::MockBackend>),
}

impl Manager {
    fn play(&mut self, data: StaticSoundData) -> Result<StaticSoundHandle, String> {
        match self {
            Self::Default(manager) => manager.play(data).map_err(|e| e.to_string()),
            #[cfg(test)]
            Self::Mock(manager) => manager.play(data).map_err(|e| e.to_string()),
        }
    }

    fn set_main_volume(&mut self, volume: Decibels) {
        match self {
            Self::Default(manager) => manager.main_track().set_volume(volume, Tween::default()),
            #[cfg(test)]
            Self::Mock(manager) => manager.main_track().set_volume(volume, Tween::default()),
        }
    }
}

/// The live mixer and the one loop channel it owns.
struct Device {
    /// The `kira` mixer. Dropping it stops all output, so it lives as long as the
    /// [`Audio`] that owns it.
    manager: Manager,
    /// The playing loop's `kira` handle, kept so it can be stopped or replaced.
    /// Held only in device mode — [`Audio::looping`] is the device-independent
    /// view of the same fact.
    looping: Option<StaticSoundHandle>,
}

/// The engine's audio layer: load sounds, play them once or looping, set the
/// master level.
///
/// # The operations
///
/// [`load`](Self::load) a file into a [`SoundHandle`] (once per path);
/// [`play_once`](Self::play_once) for a discrete event;
/// [`play_looping`](Self::play_looping) and [`stop_looping`](Self::stop_looping)
/// for the one continuous bed, typically background music;
/// [`play_layer`](Self::play_layer), [`set_layer_volume`](Self::set_layer_volume),
/// [`set_layer_rate`](Self::set_layer_rate) and [`stop_layer`](Self::stop_layer)
/// for more loops under it, each faded, and sped up or slowed, on its own; and
/// [`set_master_volume`](Self::set_master_volume) for everything at once. There
/// is deliberately nothing else — no mixer graph, no effects, no spatialisation
/// (all out of scope for this crate).
///
/// # Layers
///
/// A **layer** is a sound looping on its own, alongside the loop channel and any
/// other layers: an ambient bed under the music, an engine's drone, a second
/// drone faded in and out over it. [`play_layer`](Self::play_layer) starts one
/// and hands back a [`LayerHandle`]; [`set_layer_volume`](Self::set_layer_volume)
/// moves its level, over a fade of any length, while it plays;
/// [`set_layer_rate`](Self::set_layer_rate) glides its playback rate, and so its
/// pitch, the way an engine's note rises as it revs; and
/// [`stop_layer`](Self::stop_layer) fades it out and ends it. Unlike the loop
/// channel, layers are not idempotent: each `play_layer` starts a new one, since
/// the caller holds the handle that says which is which. A stopped layer's
/// handle is ignored from then on.
///
/// # One loop channel
///
/// There is exactly **one** looping slot. [`play_looping`](Self::play_looping) on
/// the sound that already owns it is a **no-op**, and on a different sound it
/// *replaces* it (stopping the old one). So a caller that re-asks for its loop —
/// after a state change, on a restart, every frame if it likes — can never end up
/// with two copies of it playing over each other. That invariant lives here
/// rather than in each game because "I asked for my music twice" is a mistake
/// every game would otherwise make once.
///
/// # No device required, no panics, no branches at the call site
///
/// Constructing a `kira` mixer fails when the machine has no audio output —
/// headless CI, a remote shell, the `--smoke` oracle.
/// [`with_output_device`](Self::with_output_device) turns that failure into
/// **silent mode** instead of an error, and [`silent`](Self::silent) constructs it
/// directly. In silent mode every operation is accepted and does nothing audible:
/// loads still return valid handles (nothing is decoded), plays are counted and
/// dropped, volume is remembered. Nothing in this crate panics, unwraps, or
/// returns a `Result`, so game code calls `play_once` unconditionally — no `cfg`,
/// no `if audio_available`, no error to swallow.
///
/// # Assertable without a device
///
/// Because silence is a behaviour and not an absence, it has to be *provable*.
/// `Audio` therefore records what it was asked to do, the way
/// `stickshift-render-api`'s `NullRenderer` records draws, and
/// [`is_silent`](Self::is_silent) says which mode it is in.
/// [`one_shots_played`](Self::one_shots_played),
/// [`one_shots_of`](Self::one_shots_of), [`sound_count`](Self::sound_count) and
/// [`master_volume`](Self::master_volume) hold the same values in both modes, so
/// a plain `cargo test` on a machine with no output device can assert exactly
/// what a game asked to hear. [`looping`](Self::looping) and
/// [`loop_starts`](Self::loop_starts) do too, as long as every sound decoded: on a
/// device, a sound whose file failed to decode never takes the loop channel,
/// while silent mode, which decodes nothing, lets any sound take it. And
/// [`decode_count`](Self::decode_count) is always `0` in silent mode.
///
/// # Off the hot path
///
/// Decoding happens in [`load`](Self::load) and nowhere else, so no play call
/// touches the filesystem. The play calls do no work proportional to the sound
/// and allocate nothing per frame — they are meant to be driven by game *events*
/// (a state change, a collision), not polled every update.
pub struct Audio {
    output: Output,
    /// This layer's id, carried by every handle it issues.
    owner: u32,
    /// Loaded sounds, indexed by [`SoundHandle`]'s wrapped value.
    sounds: Vec<Sound>,
    /// Path → handle, so a second load of the same file is a lookup.
    by_path: HashMap<PathBuf, SoundHandle>,
    /// How many files were actually decoded, for dedup assertions.
    decodes: usize,
    /// Which sound owns the loop channel, if any.
    looping: Option<SoundHandle>,
    /// How many times a loop actually *started* (as opposed to a repeat request
    /// for the sound already looping, which starts nothing).
    loop_starts: u32,
    /// How many one-shots have been requested across all sounds.
    one_shots_played: u32,
    /// The current master level.
    master: Volume,
    /// The layer table: a slot per layer playing, `None` once it stopped.
    layers: Vec<Option<Layer>>,
    /// Each slot's generation, bumped as its layer stops.
    layer_generations: Vec<u32>,
    /// How many layers have been started.
    layer_starts: u32,
}

impl Audio {
    /// An audio layer bound to the system's default output device, falling back
    /// to [`silent`](Self::silent) if there isn't one.
    ///
    /// Never fails and never panics: a machine with no output device (or a device
    /// that refuses to open) gets the silent no-op, with one line on stderr
    /// saying so. This is what the windowed engine entry uses.
    #[must_use]
    pub fn with_output_device() -> Self {
        let output = match AudioManager::<DefaultBackend>::new(AudioManagerSettings::default()) {
            Ok(manager) => Output::Device(Box::new(Device {
                manager: Manager::Default(manager),
                looping: None,
            })),
            // The whole point of the fallback: no device is a perfectly ordinary
            // state (CI, a remote shell), not an error the game should handle.
            Err(error) => {
                eprintln!("stickshift-audio: no audio output device ({error}); running silent");
                Output::Silent
            }
        };
        Self::with_output(output)
    }

    /// An audio layer that opens **no device** and produces no sound.
    ///
    /// Everything still works: loads hand back valid handles, plays are accepted
    /// and recorded, volume is remembered. This is what the engine's headless
    /// driver holds, so tests and the `--smoke` oracle never touch (or wait on)
    /// an audio device, and it is constructible directly — not merely reachable by
    /// a device failure — so a test can exercise the silent path deterministically
    /// on a machine that *does* have speakers.
    #[must_use]
    pub fn silent() -> Self {
        Self::with_output(Output::Silent)
    }

    /// Shared constructor for both modes.
    fn with_output(output: Output) -> Self {
        Self {
            output,
            owner: NEXT_OWNER.fetch_add(1, Ordering::Relaxed),
            sounds: Vec::new(),
            by_path: HashMap::new(),
            decodes: 0,
            looping: None,
            loop_starts: 0,
            one_shots_played: 0,
            master: Volume::UNCHANGED,
            layers: Vec::new(),
            layer_generations: Vec::new(),
            layer_starts: 0,
        }
    }

    /// Whether this layer is in silent mode (no device, nothing audible).
    ///
    /// Game code should not need to ask — every operation works either way — but
    /// tests and diagnostics do.
    #[must_use]
    pub fn is_silent(&self) -> bool {
        matches!(self.output, Output::Silent)
    }

    /// Load the sound at `path` and return its handle.
    ///
    /// Decoding happens **here**, once per path: a second load of the same file
    /// returns the same [`SoundHandle`] without re-reading or re-decoding it, so
    /// N plays of one sound cost one decode (the engine's "load once, reference by
    /// handle" rule).
    ///
    /// Infallible by design. If the file cannot be read or decoded, that one
    /// sound becomes silent (a warning goes to stderr) and a valid handle is still
    /// returned — a missing sound must not take a game down, and callers must not
    /// have to branch. In silent mode nothing is decoded at all, so a headless run
    /// pays neither the I/O nor the memory.
    pub fn load(&mut self, path: impl AsRef<Path>) -> SoundHandle {
        let key = path.as_ref().to_path_buf();
        if let Some(&existing) = self.by_path.get(&key) {
            return existing;
        }

        let data = if self.is_silent() {
            None
        } else {
            match StaticSoundData::from_file(&key) {
                Ok(data) => {
                    self.decodes += 1;
                    Some(data)
                }
                Err(error) => {
                    eprintln!(
                        "stickshift-audio: could not load {} ({error}); this sound will be silent",
                        key.display()
                    );
                    None
                }
            }
        };

        let handle = SoundHandle::new(self.owner, self.sounds.len() as u32);
        self.sounds.push(Sound {
            path: key.clone(),
            data,
            one_shots: 0,
        });
        self.by_path.insert(key, handle);
        handle
    }

    /// Play `sound` once at `volume`, on top of whatever else is playing.
    ///
    /// The right call for a discrete event. Overlapping one-shots of the same
    /// sound mix rather than cutting each other off, and a one-shot ends by
    /// itself. Unknown handles, undecodable sounds and silent mode are all no-ops
    /// — but the request is still counted
    /// ([`one_shots_played`](Self::one_shots_played)), because what a test needs
    /// to know is what the game *asked* for.
    pub fn play_once(&mut self, sound: SoundHandle, volume: Volume) {
        let Some(index) = self.index_of(sound) else {
            return;
        };
        self.sounds[index].one_shots += 1;
        self.one_shots_played += 1;

        let Output::Device(device) = &mut self.output else {
            return;
        };
        let Some(data) = &self.sounds[index].data else {
            return;
        };
        // `volume` returns a cheap clone (the samples live behind an `Arc`), so a
        // one-shot copies no audio.
        let shot = data.volume(Decibels(volume.0));
        if let Err(error) = device.manager.play(shot) {
            eprintln!("stickshift-audio: could not play a sound ({error})");
        }
    }

    /// Start `sound` looping at `volume` on the single loop channel.
    ///
    /// The whole sound is the loop region, so it repeats seamlessly and forever
    /// until [`stop_looping`](Self::stop_looping) replaces or ends it.
    ///
    /// **Idempotent for the sound already looping:** if `sound` currently owns the
    /// loop channel this does nothing at all — it does not restart the sound and
    /// does not layer a second copy. Asking for a *different* sound stops the
    /// current loop and starts the new one. So the count of loops actually started
    /// ([`loop_starts`](Self::loop_starts)) rises only when playback really began,
    /// which is what a test asserting "the music was never doubled" pins.
    ///
    /// A sound that failed to load cannot loop, so asking for one leaves the
    /// current loop playing rather than replacing it with silence.
    pub fn play_looping(&mut self, sound: SoundHandle, volume: Volume) {
        let Some(index) = self.index_of(sound) else {
            return;
        };
        if self.looping == Some(sound) {
            return;
        }

        // A sound that failed to load does not take the channel (on a device;
        // silent mode decodes nothing and keeps the channel's state for any
        // sound): it would claim to be looping while silent, and a later request
        // for the same sound would then do nothing. It is checked *before* the
        // current loop stops, so a bad file never turns the music off.
        if !self.is_silent() && self.sounds[index].data.is_none() {
            return;
        }

        // One loop channel: whatever was looping gives it up first.
        self.stop_looping();

        let Output::Device(device) = &mut self.output else {
            // Silent mode: no audio, but the loop channel's state is kept.
            self.looping = Some(sound);
            self.loop_starts += 1;
            return;
        };
        let Some(data) = &self.sounds[index].data else {
            return;
        };
        // `..` is the whole sound as the loop region, so it repeats end-to-start.
        let looped = data.loop_region(..).volume(Decibels(volume.0));
        match device.manager.play(looped) {
            Ok(playing) => {
                device.looping = Some(playing);
                self.looping = Some(sound);
                self.loop_starts += 1;
            }
            Err(error) => eprintln!("stickshift-audio: could not start a looping sound ({error})"),
        }
    }

    /// Stop whatever owns the loop channel, leaving it free.
    ///
    /// A no-op when nothing is looping. One-shots already playing are untouched.
    pub fn stop_looping(&mut self) {
        self.looping = None;
        if let Output::Device(device) = &mut self.output {
            if let Some(mut playing) = device.looping.take() {
                // The stop command is published to the audio thread by `stop`
                // itself, so dropping the handle right after is safe.
                playing.stop(Tween::default());
            }
        }
    }

    /// Start `sound` looping as a new **layer** at `volume`, alongside the loop
    /// channel and any other layers, and return the handle that controls it.
    ///
    /// The whole sound is the loop region, so it repeats seamlessly until
    /// [`stop_layer`](Self::stop_layer). Each call starts a new layer, even of a
    /// sound already layered. In silent mode, and for a sound that failed to
    /// load, nothing is heard but the layer is still kept, so its volume can be
    /// set and asserted like any other. An unknown sound handle starts nothing
    /// and returns a handle that names no layer.
    pub fn play_layer(&mut self, sound: SoundHandle, volume: Volume) -> LayerHandle {
        let Some(index) = self.index_of(sound) else {
            return LayerHandle::new(self.owner, u32::MAX, 0);
        };
        let playing = match (&mut self.output, &self.sounds[index].data) {
            (Output::Device(device), Some(data)) => {
                let looped = data.loop_region(..).volume(Decibels(volume.0));
                match device.manager.play(looped) {
                    Ok(playing) => Some(playing),
                    Err(error) => {
                        eprintln!("stickshift-audio: could not start a layer ({error})");
                        None
                    }
                }
            }
            _ => None,
        };
        let layer = Layer {
            sound,
            volume,
            rate: 1.0,
            playing,
        };
        let slot = match self.layers.iter().position(Option::is_none) {
            Some(slot) => {
                self.layers[slot] = Some(layer);
                slot
            }
            None => {
                self.layers.push(Some(layer));
                self.layer_generations.push(0);
                self.layers.len() - 1
            }
        };
        self.layer_starts += 1;
        LayerHandle::new(self.owner, slot as u32, self.layer_generations[slot])
    }

    /// Move `layer` to `volume` over a fade of `fade` (zero for at once).
    ///
    /// A fade already under way is taken over from wherever it has got to.
    /// The new level is remembered (see [`layer_volume`](Self::layer_volume))
    /// in silent mode too. A stopped or unknown layer is ignored.
    pub fn set_layer_volume(&mut self, layer: LayerHandle, volume: Volume, fade_over: Duration) {
        let Some(slot) = self.layer_slot(layer) else {
            return;
        };
        let Some(playing) = self.layers[slot].as_mut() else {
            return;
        };
        playing.volume = volume;
        if let Some(sound) = &mut playing.playing {
            sound.set_volume(Decibels(volume.0), fade(fade_over));
        }
    }

    /// Move `layer`'s playback rate to `rate` over a glide of `glide_over`
    /// (zero for at once): `1.0` plays it as recorded, `2.0` twice as fast and
    /// an octave up, `0.5` half as fast and an octave down. Speed and pitch go
    /// together, as on a tape.
    ///
    /// A glide already under way is taken over from wherever it has got to.
    /// The new rate is remembered (see [`layer_rate`](Self::layer_rate)) in
    /// silent mode too. A rate that is not a positive, finite number is
    /// ignored, as is a stopped or unknown layer.
    pub fn set_layer_rate(&mut self, layer: LayerHandle, rate: f64, glide_over: Duration) {
        if !(rate.is_finite() && rate > 0.0) {
            return;
        }
        let Some(slot) = self.layer_slot(layer) else {
            return;
        };
        let Some(playing) = self.layers[slot].as_mut() else {
            return;
        };
        playing.rate = rate;
        if let Some(sound) = &mut playing.playing {
            sound.set_playback_rate(PlaybackRate(rate), fade(glide_over));
        }
    }

    /// Fade `layer` out over `fade` and end it. Its handle is ignored from then
    /// on. A stopped or unknown layer is ignored.
    pub fn stop_layer(&mut self, layer: LayerHandle, fade_over: Duration) {
        let Some(slot) = self.layer_slot(layer) else {
            return;
        };
        if let Some(Layer {
            playing: Some(mut sound),
            ..
        }) = self.layers[slot].take()
        {
            sound.stop(fade(fade_over));
        }
        self.layer_generations[slot] = self.layer_generations[slot].wrapping_add(1);
    }

    /// The level `layer` was last set to, or `None` if it has stopped (or never
    /// was).
    #[must_use]
    pub fn layer_volume(&self, layer: LayerHandle) -> Option<Volume> {
        let slot = self.layer_slot(layer)?;
        self.layers[slot].as_ref().map(|playing| playing.volume)
    }

    /// The playback rate `layer` was last set to (`1.0` until
    /// [`set_layer_rate`](Self::set_layer_rate)), or `None` if it has stopped
    /// (or never was).
    #[must_use]
    pub fn layer_rate(&self, layer: LayerHandle) -> Option<f64> {
        let slot = self.layer_slot(layer)?;
        self.layers[slot].as_ref().map(|playing| playing.rate)
    }

    /// The sound `layer` loops, or `None` if it has stopped (or never was).
    #[must_use]
    pub fn layer_sound(&self, layer: LayerHandle) -> Option<SoundHandle> {
        let slot = self.layer_slot(layer)?;
        self.layers[slot].as_ref().map(|playing| playing.sound)
    }

    /// How many layers are playing.
    #[must_use]
    pub fn layer_count(&self) -> usize {
        self.layers.iter().filter(|slot| slot.is_some()).count()
    }

    /// How many layers have been started in all.
    #[must_use]
    pub fn layer_starts(&self) -> u32 {
        self.layer_starts
    }

    /// Validate a layer handle: this layer's, and the start of its slot that is
    /// playing now.
    fn layer_slot(&self, layer: LayerHandle) -> Option<usize> {
        let slot = layer.index() as usize;
        (layer.owner() == self.owner
            && self.layer_generations.get(slot) == Some(&layer.generation())
            && self.layers[slot].is_some())
        .then_some(slot)
    }

    /// Set the level everything is mixed through.
    ///
    /// Applies to sounds already playing as well as later ones, and is remembered
    /// (see [`master_volume`](Self::master_volume)) in silent mode too.
    pub fn set_master_volume(&mut self, volume: Volume) {
        self.master = volume;
        if let Output::Device(device) = &mut self.output {
            device.manager.set_main_volume(Decibels(volume.0));
        }
    }

    /// The current master level, as last set (default
    /// [`Volume::UNCHANGED`]).
    #[must_use]
    pub fn master_volume(&self) -> Volume {
        self.master
    }

    /// Which sound owns the loop channel, or `None` if nothing is looping.
    #[must_use]
    pub fn looping(&self) -> Option<SoundHandle> {
        self.looping
    }

    /// How many times a loop actually **started**.
    ///
    /// Repeat requests for the sound already looping do not count (they start
    /// nothing), so this is the number a test compares against 1 to prove a loop
    /// was never layered on top of itself.
    #[must_use]
    pub fn loop_starts(&self) -> u32 {
        self.loop_starts
    }

    /// Total one-shots requested across all sounds.
    #[must_use]
    pub fn one_shots_played(&self) -> u32 {
        self.one_shots_played
    }

    /// One-shots requested for `sound` specifically (`0` for an unknown handle).
    ///
    /// This is how a caller proves an event-driven sound fired exactly as often
    /// as the event happened — no more, no less.
    #[must_use]
    pub fn one_shots_of(&self, sound: SoundHandle) -> u32 {
        self.index_of(sound)
            .map_or(0, |index| self.sounds[index].one_shots)
    }

    /// Number of distinct sounds loaded (one per distinct path).
    #[must_use]
    pub fn sound_count(&self) -> usize {
        self.sounds.len()
    }

    /// Number of files actually decoded.
    ///
    /// With the load-once cache this equals the number of distinct paths that
    /// decoded successfully, so a second [`load`](Self::load) of a path leaves it
    /// unchanged. Always `0` in silent mode, where nothing is decoded at all.
    #[must_use]
    pub fn decode_count(&self) -> usize {
        self.decodes
    }

    /// The path `sound` was loaded from, or `None` for an unknown handle.
    #[must_use]
    pub fn path(&self, sound: SoundHandle) -> Option<&Path> {
        self.index_of(sound)
            .map(|index| self.sounds[index].path.as_path())
    }

    /// Validate a handle against this layer and its sound table.
    ///
    /// Every operation goes through this, which is what makes a stale or foreign
    /// handle a no-op instead of a panic, or a wrong sound.
    fn index_of(&self, sound: SoundHandle) -> Option<usize> {
        let index = sound.index() as usize;
        (sound.owner() == self.owner && index < self.sounds.len()).then_some(index)
    }
}

impl Default for Audio {
    /// [`Audio::silent`] — the mode that needs nothing from the machine.
    ///
    /// Deliberately *not* the device-opening constructor: `Default` is what gets
    /// reached for implicitly, and implicitly opening an output device (in a test,
    /// in a tool, on a build machine) is exactly the surprise this crate exists to
    /// avoid. Opening a device is always spelled out:
    /// [`with_output_device`](Self::with_output_device).
    fn default() -> Self {
        Self::silent()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A path that does not exist. Silent mode never opens it (nothing is
    /// decoded), which is what lets these tests run anywhere.
    const A: &str = "/stickshift-audio/tests/does-not-exist-a.wav";
    /// A second such path.
    const B: &str = "/stickshift-audio/tests/does-not-exist-b.wav";

    #[test]
    fn silent_mode_accepts_everything_and_decodes_nothing() {
        // The load-bearing property: with no device, the full API still works and
        // hands back usable handles, so game code needs no second path.
        let mut audio = Audio::silent();
        assert!(audio.is_silent());
        assert!(
            Audio::default().is_silent(),
            "the default never opens a device"
        );

        let sound = audio.load(A);
        assert_eq!(audio.sound_count(), 1);
        assert_eq!(
            audio.decode_count(),
            0,
            "silent mode decodes nothing at all"
        );
        assert_eq!(audio.path(sound), Some(Path::new(A)));

        audio.play_once(sound, Volume(-3.0));
        audio.play_looping(sound, Volume(-9.0));
        audio.set_master_volume(Volume(-6.0));
        audio.stop_looping();

        // Every request was still recorded, so silence is assertable.
        assert_eq!(audio.one_shots_played(), 1);
        assert_eq!(audio.loop_starts(), 1);
        assert_eq!(audio.master_volume(), Volume(-6.0));
        assert_eq!(audio.looping(), None);
    }

    #[test]
    fn loading_the_same_path_twice_reuses_the_handle() {
        // Asset-cache discipline: one path, one handle, one decode.
        let mut audio = Audio::silent();
        let first = audio.load(A);
        let second = audio.load(A);
        assert_eq!(first, second, "the same path yields the same handle");
        assert_eq!(audio.sound_count(), 1, "and no second entry");

        // A different path is a different sound.
        let other = audio.load(B);
        assert_ne!(first, other);
        assert_eq!(audio.sound_count(), 2);
    }

    #[test]
    fn one_shots_are_counted_per_sound_and_in_total() {
        let mut audio = Audio::silent();
        let a = audio.load(A);
        let b = audio.load(B);

        audio.play_once(a, Volume::UNCHANGED);
        audio.play_once(a, Volume::UNCHANGED);
        audio.play_once(b, Volume::UNCHANGED);

        assert_eq!(audio.one_shots_of(a), 2);
        assert_eq!(audio.one_shots_of(b), 1);
        assert_eq!(audio.one_shots_played(), 3);
    }

    #[test]
    fn asking_again_for_the_sound_already_looping_starts_nothing() {
        // The engine-level guarantee against a doubled loop: a repeat request is
        // not a restart and not a second copy.
        let mut audio = Audio::silent();
        let a = audio.load(A);

        audio.play_looping(a, Volume(-10.0));
        assert_eq!(audio.loop_starts(), 1);
        assert_eq!(audio.looping(), Some(a));

        for _ in 0..100 {
            audio.play_looping(a, Volume(-10.0));
        }
        assert_eq!(
            audio.loop_starts(),
            1,
            "a hundred repeat requests started nothing"
        );
        assert_eq!(audio.looping(), Some(a));
    }

    #[test]
    fn a_different_sound_takes_over_the_one_loop_channel() {
        let mut audio = Audio::silent();
        let a = audio.load(A);
        let b = audio.load(B);

        audio.play_looping(a, Volume::UNCHANGED);
        audio.play_looping(b, Volume::UNCHANGED);
        assert_eq!(audio.looping(), Some(b), "the loop channel holds one sound");
        assert_eq!(audio.loop_starts(), 2, "the replacement really started");
    }

    #[test]
    fn stopping_frees_the_channel_so_a_later_play_restarts() {
        let mut audio = Audio::silent();
        let a = audio.load(A);

        audio.play_looping(a, Volume::UNCHANGED);
        audio.stop_looping();
        assert_eq!(audio.looping(), None);

        audio.play_looping(a, Volume::UNCHANGED);
        assert_eq!(audio.looping(), Some(a));
        assert_eq!(
            audio.loop_starts(),
            2,
            "a stopped loop can be started again"
        );

        // Stopping when nothing loops is harmless.
        audio.stop_looping();
        audio.stop_looping();
        assert_eq!(audio.looping(), None);
    }

    /// An `Audio` in device mode over `kira`'s mock backend: the device-mode
    /// code paths, with no device opened and no sound produced.
    fn mock_device() -> Audio {
        let manager =
            AudioManager::<kira::backend::mock::MockBackend>::new(AudioManagerSettings::default())
                .expect("the mock backend always starts");
        Audio::with_output(Output::Device(Box::new(Device {
            manager: Manager::Mock(manager),
            looping: None,
        })))
    }

    /// Add a sound as a load would, decoded (a short silence) or, as when the
    /// file could not be read, not.
    fn add_sound(audio: &mut Audio, path: &str, decoded: bool) -> SoundHandle {
        use kira::sound::static_sound::StaticSoundSettings;
        let data = decoded.then(|| StaticSoundData {
            sample_rate: 48_000,
            frames: std::sync::Arc::from(vec![kira::Frame::ZERO; 480]),
            settings: StaticSoundSettings::default(),
            slice: None,
        });
        let handle = SoundHandle::new(audio.owner, audio.sounds.len() as u32);
        audio.sounds.push(Sound {
            path: PathBuf::from(path),
            data,
            one_shots: 0,
        });
        handle
    }

    #[test]
    fn looping_a_sound_that_failed_to_load_keeps_the_current_loop() {
        let mut audio = mock_device();
        assert!(!audio.is_silent());
        let music = add_sound(&mut audio, A, true);
        let broken = add_sound(&mut audio, B, false);

        audio.play_looping(music, Volume::UNCHANGED);
        assert_eq!((audio.looping(), audio.loop_starts()), (Some(music), 1));

        // The broken file cannot loop, and must not silence the music.
        audio.play_looping(broken, Volume::UNCHANGED);
        assert_eq!(audio.looping(), Some(music), "the music keeps playing");
        assert_eq!(audio.loop_starts(), 1, "and nothing else started");
    }

    #[test]
    fn layers_play_alongside_the_loop_channel_and_each_other() {
        let mut audio = Audio::silent();
        let (music, drone) = (audio.load(A), audio.load(B));
        audio.play_looping(music, Volume(-14.0));
        let low = audio.play_layer(drone, Volume(-20.0));
        let high = audio.play_layer(drone, Volume::SILENT);
        assert_ne!(low, high, "each play starts a layer of its own");
        assert_eq!(audio.layer_count(), 2);
        assert_eq!(
            audio.looping(),
            Some(music),
            "the loop channel is untouched"
        );
        assert_eq!(audio.layer_sound(high), Some(drone));

        audio.set_layer_volume(high, Volume(-6.0), Duration::from_millis(250));
        assert_eq!(audio.layer_volume(high), Some(Volume(-6.0)));
        assert_eq!(
            audio.layer_volume(low),
            Some(Volume(-20.0)),
            "the other is not"
        );

        audio.stop_looping();
        assert_eq!(
            audio.layer_count(),
            2,
            "stopping the music leaves the layers"
        );
        audio.stop_layer(low, Duration::from_millis(500));
        assert_eq!(audio.layer_count(), 1);
        assert_eq!(audio.layer_volume(low), None, "a stopped layer is gone");
        assert_eq!(audio.layer_starts(), 2);
    }

    #[test]
    fn a_layer_glides_to_a_rate_of_its_own_and_ignores_one_that_is_not_a_rate() {
        let mut audio = Audio::silent();
        let drone = audio.load(A);
        let low = audio.play_layer(drone, Volume(-20.0));
        let high = audio.play_layer(drone, Volume(-20.0));
        assert_eq!(
            audio.layer_rate(low),
            Some(1.0),
            "as recorded to begin with"
        );

        audio.set_layer_rate(high, 1.5, Duration::from_millis(800));
        assert_eq!(audio.layer_rate(high), Some(1.5));
        assert_eq!(audio.layer_rate(low), Some(1.0), "the other is not");
        assert_eq!(
            audio.layer_volume(high),
            Some(Volume(-20.0)),
            "nor its level"
        );

        for wrong in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            audio.set_layer_rate(high, wrong, Duration::ZERO);
            assert_eq!(audio.layer_rate(high), Some(1.5), "{wrong} is not a rate");
        }

        audio.stop_layer(high, Duration::ZERO);
        audio.set_layer_rate(high, 2.0, Duration::ZERO);
        assert_eq!(audio.layer_rate(high), None, "a stopped layer is gone");
        let next = audio.play_layer(drone, Volume::SILENT);
        assert_eq!(
            audio.layer_rate(next),
            Some(1.0),
            "its slot's next is as recorded"
        );
    }

    #[test]
    fn a_stopped_layers_handle_never_reaches_the_next_layer_in_its_slot() {
        let mut audio = Audio::silent();
        let a = audio.load(A);
        let first = audio.play_layer(a, Volume(-10.0));
        audio.stop_layer(first, Duration::ZERO);
        let second = audio.play_layer(a, Volume(-30.0));
        assert_ne!(first, second);
        audio.set_layer_volume(first, Volume::UNCHANGED, Duration::ZERO);
        audio.stop_layer(first, Duration::ZERO);
        assert_eq!(audio.layer_volume(second), Some(Volume(-30.0)), "untouched");
        assert_eq!(audio.layer_count(), 1);
    }

    #[test]
    fn layers_of_an_unknown_or_foreign_sound_or_handle_are_ignored() {
        let mut audio = Audio::silent();
        let alien = Audio::silent().load(A);
        let nothing = audio.play_layer(alien, Volume::UNCHANGED);
        assert_eq!(audio.layer_count(), 0);
        assert_eq!(audio.layer_volume(nothing), None);

        let mine = audio.load(B);
        let layer = audio.play_layer(mine, Volume(-12.0));
        let mut other = Audio::silent();
        other.set_layer_volume(layer, Volume::UNCHANGED, Duration::ZERO);
        other.stop_layer(layer, Duration::ZERO);
        assert_eq!(audio.layer_volume(layer), Some(Volume(-12.0)));
        assert_eq!(other.layer_count(), 0);
    }

    #[test]
    fn layers_play_fade_and_stop_on_a_device() {
        let mut audio = mock_device();
        let drone = add_sound(&mut audio, A, true);
        let broken = add_sound(&mut audio, B, false);
        let layer = audio.play_layer(drone, Volume(-18.0));
        assert!(
            matches!(
                &audio.layers[0],
                Some(Layer {
                    playing: Some(_),
                    ..
                })
            ),
            "a decoded sound plays"
        );
        audio.set_layer_volume(layer, Volume(-3.0), Duration::from_millis(300));
        assert_eq!(audio.layer_volume(layer), Some(Volume(-3.0)));
        audio.set_layer_rate(layer, 1.25, Duration::from_millis(300));
        assert_eq!(audio.layer_rate(layer), Some(1.25));
        let quiet = audio.play_layer(broken, Volume(-18.0));
        audio.set_layer_rate(quiet, 0.8, Duration::ZERO);
        assert_eq!(
            audio.layer_rate(quiet),
            Some(0.8),
            "kept, silent, its rate too"
        );
        assert_eq!(audio.layer_count(), 2, "a broken sound is kept, silent");
        audio.stop_layer(layer, Duration::from_millis(100));
        audio.stop_layer(quiet, Duration::ZERO);
        assert_eq!(audio.layer_count(), 0);
    }

    #[test]
    fn a_handle_from_another_audio_is_ignored() {
        let mut audio = Audio::silent();
        let mine = audio.load(A);
        // The first sound of another layer: the same table index as `mine`.
        let theirs = Audio::silent().load(B);
        assert_ne!(mine, theirs);

        audio.play_once(theirs, Volume::UNCHANGED);
        audio.play_looping(theirs, Volume::UNCHANGED);
        assert_eq!(audio.one_shots_played(), 0, "not taken for `mine`");
        assert_eq!(audio.one_shots_of(mine), 0);
        assert_eq!(audio.looping(), None);
        assert_eq!(audio.path(theirs), None);
    }

    #[test]
    fn an_unknown_handle_is_ignored_rather_than_fatal() {
        // Handles from another `Audio` (or a stale one) must not take a game down.
        let mut audio = Audio::silent();
        let alien = Audio::silent().load(A);

        audio.play_once(alien, Volume::UNCHANGED);
        audio.play_looping(alien, Volume::UNCHANGED);

        assert_eq!(audio.one_shots_played(), 0);
        assert_eq!(audio.one_shots_of(alien), 0);
        assert_eq!(audio.loop_starts(), 0);
        assert_eq!(audio.looping(), None);
        assert_eq!(audio.path(alien), None);
    }
}
