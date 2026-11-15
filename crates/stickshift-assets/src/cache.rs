// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

//! Load-once / dedup [`AssetCache`]: parse + upload a glTF file **once**, then
//! share it by handle across N instances (the renderer's upload-once discipline, one level up).
//!
//! The cache keys imported scenes by their file path, canonicalized, so
//! `assets/model.glb`, `./assets/model.glb` and the file's absolute path (or a
//! symlink to it) are one entry. The first request for a file parses it and
//! (optionally) uploads its meshes into the render device, storing the resulting
//! [`MeshHandle`]s; every later request for the same file returns the
//! already-parsed scene and the already-uploaded handles without parsing or
//! uploading again. This is the
//! "upload once, reference by handle" rule applied to whole asset files.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use stickshift_render_api::{MeshData, MeshHandle, RenderDevice};

use crate::error::ImportError;
use crate::import::import_gltf;
use crate::scene::SceneAsset;

/// A cached, uploaded asset: the parsed scene plus one [`MeshHandle`] per mesh in
/// [`SceneAsset::meshes`] (parallel by index), if the caller uploaded it.
#[derive(Debug, Clone)]
pub struct CachedAsset {
    /// The parsed scene (shared; cheap to clone via the `Arc`).
    pub scene: Arc<SceneAsset>,
    /// One GPU [`MeshHandle`] per `scene.meshes[i]`, in the same order. Empty if
    /// the asset was loaded parse-only (no device upload).
    pub mesh_handles: Vec<MeshHandle>,
}

/// A load-once cache of imported glTF scenes, keyed by canonical file path.
///
/// A file parses (and, via [`load`](Self::load), uploads) exactly once no matter
/// how many times it is requested. Cloning the returned [`CachedAsset`] is cheap
/// (the scene is behind an `Arc`), so N game instances share one parse + one
/// GPU upload.
#[derive(Default)]
pub struct AssetCache {
    entries: HashMap<PathBuf, CachedAsset>,
    /// Parse counter — how many times a file was actually read+parsed (as opposed
    /// to served from cache). Used by tests to prove dedup.
    parse_count: usize,
}

impl AssetCache {
    /// Create an empty cache.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Parse `path` into a [`SceneAsset`] once and cache it, **without** any GPU
    /// upload (the returned [`CachedAsset::mesh_handles`] is empty). Later calls
    /// for the same file, by any path, return the cached scene without
    /// re-parsing.
    ///
    /// # Errors
    /// Returns [`ImportError`] if a first-time parse of `path` fails. The error
    /// names the path as given.
    pub fn load_parse_only(&mut self, path: impl AsRef<Path>) -> Result<CachedAsset, ImportError> {
        let path = path.as_ref();
        let key = cache_key(path);
        if let Some(existing) = self.entries.get(&key) {
            return Ok(existing.clone());
        }
        let scene = import_gltf(path)?;
        self.parse_count += 1;
        let cached = CachedAsset {
            scene: Arc::new(scene),
            mesh_handles: Vec::new(),
        };
        self.entries.insert(key, cached.clone());
        Ok(cached)
    }

    /// Parse `path` and upload its meshes into `device` once, caching both the
    /// scene and the resulting [`MeshHandle`]s. Later calls for the same file, by
    /// any path, return the cached handles without re-parsing or re-uploading.
    ///
    /// If the path was previously loaded parse-only, this uploads its meshes now
    /// and upgrades the cache entry in place.
    ///
    /// # Errors
    /// Returns [`ImportError`] if a first-time parse of `path` fails.
    pub fn load<D: RenderDevice + ?Sized>(
        &mut self,
        device: &mut D,
        path: impl AsRef<Path>,
    ) -> Result<CachedAsset, ImportError> {
        let path = path.as_ref();
        let key = cache_key(path);

        // Already uploaded → serve as-is.
        if let Some(existing) = self.entries.get(&key) {
            if !existing.mesh_handles.is_empty() || existing.scene.meshes.is_empty() {
                return Ok(existing.clone());
            }
        }

        // Parse once (reuse a parse-only entry if present).
        let scene = match self.entries.get(&key) {
            Some(existing) => Arc::clone(&existing.scene),
            None => {
                let parsed = import_gltf(path)?;
                self.parse_count += 1;
                Arc::new(parsed)
            }
        };

        // Upload every mesh once.
        let mesh_handles = upload_scene(device, &scene);
        let cached = CachedAsset {
            scene,
            mesh_handles,
        };
        self.entries.insert(key, cached.clone());
        Ok(cached)
    }

    /// Number of files actually parsed (read from disk), for dedup assertions.
    #[must_use]
    pub fn parse_count(&self) -> usize {
        self.parse_count
    }

    /// Number of distinct cached files.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the cache holds no entries.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// The cache key for `path`: its canonical form, so every path to one file
/// (relative, absolute, through `.`/`..` or a symlink) is one entry. A path
/// that cannot be canonicalized (a missing file) is its own key, so the import
/// that follows reports the path the caller gave.
fn cache_key(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Upload every mesh of `scene` into `device`, returning one handle per mesh.
///
/// `D` is `?Sized` so a trait-object device (e.g. `&mut dyn Renderer` from the
/// engine's render seam) can drive the upload directly.
pub fn upload_scene<D: RenderDevice + ?Sized>(
    device: &mut D,
    scene: &SceneAsset,
) -> Vec<MeshHandle> {
    scene
        .meshes
        .iter()
        .map(|mesh| {
            device.create_mesh(&MeshData {
                vertices: &mesh.vertices,
                indices: &mesh.indices,
                layout: mesh.layout.clone(),
            })
        })
        .collect()
}
