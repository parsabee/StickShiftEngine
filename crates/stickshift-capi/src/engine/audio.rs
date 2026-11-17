// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

//! The engine's audio through C: kira on the default output device, or silent
//! with the same counters (`STICKSHIFT_AUDIO_SILENT`, what tools and tests use).
//!
//! A [`StickShiftSound`] is a generational handle of its own kind over the
//! engine's sound table. Rust's `Audio` has no unload, so a sound lives until
//! the engine is destroyed. The table carries a tag the engine takes from the
//! contexts' pool, so a sound from another engine, or from an earlier engine on
//! the same context, is `STICKSHIFT_ERROR_INVALID_HANDLE` rather than a sound of
//! this one.

use std::collections::HashMap;
use std::ffi::{c_char, CString};

use stickshift_core::{Audio, SoundHandle, Volume};

use super::{game_call, Access, StickShiftEngine};
use crate::core::{output, utf8, write_sized, Error};
use crate::handles::{ContextTag, Kind, Slots, MAX_CONTEXTS};
use crate::types::*;

fn not_a_sound() -> Error {
    Error::new(
        STICKSHIFT_ERROR_INVALID_HANDLE,
        "sound is not a sound this engine loaded",
    )
}

/// One loaded sound.
struct Sound {
    handle: SoundHandle,
    /// The path it was first loaded by, which keys it (`Audio::path`).
    path: CString,
}

/// An engine's sounds, by C handle.
pub(crate) struct Sounds {
    table: Slots<Sound>,
    /// The C handle of each Rust one: a path loaded again returns its sound.
    ids: HashMap<SoundHandle, u64>,
    /// The tag the handles carry, held for the engine's life.
    _tag: ContextTag,
}

impl Sounds {
    /// An empty table, or `STICKSHIFT_ERROR_LIMIT_EXCEEDED` if every tag is
    /// taken.
    pub(crate) fn new() -> Result<Self, Error> {
        let tag = ContextTag::acquire().ok_or_else(|| {
            Error::new(
                STICKSHIFT_ERROR_LIMIT_EXCEEDED,
                format!("{MAX_CONTEXTS} contexts and engines are already alive; destroy one first"),
            )
        })?;
        Ok(Self {
            table: Slots::new(Kind::Sound, &tag),
            ids: HashMap::new(),
            _tag: tag,
        })
    }

    /// The Rust sound `sound` names.
    fn handle(&self, sound: StickShiftSound) -> Result<SoundHandle, Error> {
        self.table
            .get(sound.id)
            .map(|s| s.handle)
            .ok_or_else(not_a_sound)
    }

    /// The C handle of a Rust sound this engine loaded, or 0.
    fn id(&self, handle: Option<SoundHandle>) -> u64 {
        handle.and_then(|h| self.ids.get(&h)).copied().unwrap_or(0)
    }
}

impl Access<'_> {
    /// The engine's audio, as the game reaches it.
    fn audio(&mut self) -> &mut Audio {
        match self {
            Access::Idle(state) => &mut state.session.lp.audio,
            Access::Hook(ctx) => ctx.audio(),
        }
    }
}

/// Load the sound at `path` (a file path, UTF-8): decoded now, once per path;
/// loading a path again returns the same sound. Paths compare as Rust's `Path`s
/// do: a repeated `/`, a `.` component inside the path or a trailing `/` does
/// not make a new sound, while a leading `./` does, and
/// `stickshift_audio_get_path` returns the spelling the sound was first loaded
/// by. A file that cannot be read or decoded gives a silent sound (and a line on
/// stderr), as in Rust, so this fails only for a NULL or non-UTF-8 path. Silent
/// engines decode nothing. In a hook, or between engine calls.
///
/// # Safety
/// `engine` must be NULL or a live engine; `path` NULL or a NUL-terminated
/// string; `out` NULL or valid.
#[no_mangle]
pub unsafe extern "C" fn stickshift_audio_load(
    engine: *mut StickShiftEngine,
    path: *const c_char,
    out: *mut StickShiftSound,
) -> StickShiftResult {
    game_call(engine, "stickshift_audio_load", |e, mut access| {
        let out = output(out, "out")?;
        *out = StickShiftSound::default();
        let path = utf8(path, "path")?;
        let handle = access.audio().load(path);
        let mut sounds = e.sounds.borrow_mut();
        let id = match sounds.ids.get(&handle) {
            Some(&id) => id,
            None => {
                let sound = Sound {
                    handle,
                    // A C string has no interior NUL.
                    path: CString::new(path).unwrap_or_default(),
                };
                let id = sounds
                    .table
                    .insert(sound)
                    .map_err(|_| Error::new(STICKSHIFT_ERROR_LIMIT_EXCEEDED, "too many sounds"))?;
                sounds.ids.insert(handle, id);
                id
            }
        };
        *out = StickShiftSound { id };
        Ok(STICKSHIFT_OK)
    })
}

/// Play `sound` once at `volume_db` (`STICKSHIFT_VOLUME_UNCHANGED_DB` for its
/// recorded level), over whatever else plays. Counted even when nothing is
/// heard (silent mode, a sound that failed to load). In a hook, or between
/// engine calls.
///
/// # Safety
/// `engine` must be NULL or a live engine.
#[no_mangle]
pub unsafe extern "C" fn stickshift_audio_play_once(
    engine: *mut StickShiftEngine,
    sound: StickShiftSound,
    volume_db: f32,
) -> StickShiftResult {
    game_call(engine, "stickshift_audio_play_once", |e, mut access| {
        let handle = e.sounds.borrow().handle(sound)?;
        access.audio().play_once(handle, Volume(volume_db));
        Ok(STICKSHIFT_OK)
    })
}

/// Loop `sound` at `volume_db` on the single loop channel, replacing what loops
/// now. A request for the sound already looping does nothing (it neither
/// restarts nor doubles). In a hook, or between engine calls.
///
/// # Safety
/// `engine` must be NULL or a live engine.
#[no_mangle]
pub unsafe extern "C" fn stickshift_audio_play_looping(
    engine: *mut StickShiftEngine,
    sound: StickShiftSound,
    volume_db: f32,
) -> StickShiftResult {
    game_call(engine, "stickshift_audio_play_looping", |e, mut access| {
        let handle = e.sounds.borrow().handle(sound)?;
        access.audio().play_looping(handle, Volume(volume_db));
        Ok(STICKSHIFT_OK)
    })
}

/// Stop the loop channel. Nothing looping is fine. In a hook, or between engine
/// calls.
///
/// # Safety
/// `engine` must be NULL or a live engine.
#[no_mangle]
pub unsafe extern "C" fn stickshift_audio_stop_looping(
    engine: *mut StickShiftEngine,
) -> StickShiftResult {
    game_call(engine, "stickshift_audio_stop_looping", |_, mut access| {
        access.audio().stop_looping();
        Ok(STICKSHIFT_OK)
    })
}

/// Set the level everything plays at, in dB; remembered in silent mode too. In
/// a hook, or between engine calls.
///
/// # Safety
/// `engine` must be NULL or a live engine.
#[no_mangle]
pub unsafe extern "C" fn stickshift_audio_set_master_volume(
    engine: *mut StickShiftEngine,
    volume_db: f32,
) -> StickShiftResult {
    game_call(
        engine,
        "stickshift_audio_set_master_volume",
        |_, mut access| {
            access.audio().set_master_volume(Volume(volume_db));
            Ok(STICKSHIFT_OK)
        },
    )
}

/// The audio counters: sounds loaded and decoded, loops started, one-shots
/// asked for, the master volume and what loops now. In a hook, or between
/// engine calls. Set `out->size = sizeof(StickShiftAudioStats)`.
///
/// # Safety
/// `engine` must be NULL or a live engine; `out` NULL or valid for its
/// declared size.
#[no_mangle]
pub unsafe extern "C" fn stickshift_audio_get_stats(
    engine: *mut StickShiftEngine,
    out: *mut StickShiftAudioStats,
) -> StickShiftResult {
    game_call(engine, "stickshift_audio_get_stats", |e, mut access| {
        let audio = access.audio();
        let count = |n: usize| u32::try_from(n).unwrap_or(u32::MAX);
        let stats = StickShiftAudioStats {
            size: 0,
            sound_count: count(audio.sound_count()),
            decode_count: count(audio.decode_count()),
            loop_starts: audio.loop_starts(),
            one_shots_played: audio.one_shots_played(),
            master_volume_db: audio.master_volume().0,
            looping: StickShiftSound {
                id: e.sounds.borrow().id(audio.looping()),
            },
            is_silent: u8::from(audio.is_silent()),
            reserved: [0; 7],
        };
        write_sized(out, stats, "out")?;
        Ok(STICKSHIFT_OK)
    })
}

/// How many one-shots of `sound` the game asked for. In a hook, or between
/// engine calls.
///
/// # Safety
/// `engine` must be NULL or a live engine; `out` NULL or valid.
#[no_mangle]
pub unsafe extern "C" fn stickshift_audio_one_shots_of(
    engine: *mut StickShiftEngine,
    sound: StickShiftSound,
    out: *mut u32,
) -> StickShiftResult {
    game_call(engine, "stickshift_audio_one_shots_of", |e, mut access| {
        let out = output(out, "out")?;
        let handle = e.sounds.borrow().handle(sound)?;
        *out = access.audio().one_shots_of(handle);
        Ok(STICKSHIFT_OK)
    })
}

/// The path `sound` was loaded by. The string is engine-owned and valid until
/// the engine is destroyed. In a hook, or between engine calls.
///
/// # Safety
/// `engine` must be NULL or a live engine; `out` NULL or valid.
#[no_mangle]
pub unsafe extern "C" fn stickshift_audio_get_path(
    engine: *mut StickShiftEngine,
    sound: StickShiftSound,
    out: *mut *const c_char,
) -> StickShiftResult {
    game_call(engine, "stickshift_audio_get_path", |e, _| {
        let out = output(out, "out")?;
        *out = std::ptr::null();
        let sounds = e.sounds.borrow();
        let entry = sounds.table.get(sound.id).ok_or_else(not_a_sound)?;
        *out = entry.path.as_ptr();
        Ok(STICKSHIFT_OK)
    })
}
