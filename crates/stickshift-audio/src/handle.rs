// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

//! The opaque [`SoundHandle`] a loaded sound is referenced by.
//!
//! Mirrors the render seam's handle discipline
//! (`stickshift-render-api`'s `MeshHandle` and friends): a load returns a token, and
//! everything afterwards is expressed in terms of that token. Game code stores
//! handles and never learns what a sound is made of — no sample buffer, no
//! decoder, and no `kira` type ever crosses the boundary.

/// Opaque handle to a sound loaded by [`Audio::load`](crate::mixer::Audio::load).
///
/// Consumed by [`Audio::play_once`](crate::mixer::Audio::play_once) and
/// [`Audio::play_looping`](crate::mixer::Audio::play_looping). Handles are `Copy`
/// and stay valid for the life of the [`Audio`](crate::mixer::Audio) that issued
/// them — including in silent mode, where a handle is issued for a sound that
/// was never decoded, precisely so that game code needs no second code path.
///
/// A handle names the `Audio` that issued it, so passing it to another `Audio`
/// is a no-op there rather than playing whichever sound sits at the same index.
/// Its contents are private: a handle can only come from a load.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SoundHandle {
    /// The id of the [`Audio`](crate::mixer::Audio) that issued it.
    owner: u32,
    /// Its index in that `Audio`'s sound table.
    index: u32,
}

impl SoundHandle {
    pub(crate) fn new(owner: u32, index: u32) -> Self {
        Self { owner, index }
    }

    pub(crate) fn owner(self) -> u32 {
        self.owner
    }

    pub(crate) fn index(self) -> u32 {
        self.index
    }
}

/// Opaque handle to a **layer**: a sound looping on its own, started with
/// [`Audio::play_layer`](crate::mixer::Audio::play_layer) and faded or stopped
/// through this handle.
///
/// Like a [`SoundHandle`], it names the [`Audio`](crate::mixer::Audio) that
/// issued it, so another `Audio` ignores it. It also names one start of the
/// layer: once [`Audio::stop_layer`](crate::mixer::Audio::stop_layer) has
/// stopped it, the handle is ignored, even after its slot is reused by a later
/// layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LayerHandle {
    /// The id of the [`Audio`](crate::mixer::Audio) that issued it.
    owner: u32,
    /// Its slot in that `Audio`'s layer table.
    index: u32,
    /// Which start of that slot it names.
    generation: u32,
}

impl LayerHandle {
    pub(crate) fn new(owner: u32, index: u32, generation: u32) -> Self {
        Self {
            owner,
            index,
            generation,
        }
    }

    pub(crate) fn owner(self) -> u32 {
        self.owner
    }

    pub(crate) fn index(self) -> u32 {
        self.index
    }

    pub(crate) fn generation(self) -> u32 {
        self.generation
    }
}
