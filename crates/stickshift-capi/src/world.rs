// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

//! An engine's ECS through the C API: components (`stickshift_component_*`),
//! the world (`stickshift_world_*`) and query snapshots.
//!
//! The world is the engine scene's own hecs `World`. Game components are
//! registered at run time through [`stickshift_ecs::dynamic`], so each one has a
//! Rust type of its own and the world partitions and orders archetypes exactly
//! as it does for a Rust game. The engine components keep their Rust types
//! (`TransformComponent`, `RenderComponent`, `StaticTag`, `DynamicTag`), given a
//! byte form by the codecs here, so the scene's streaming and rebase see what C
//! wrote. Nothing here adds a component of its own to any entity.
//!
//! A query copies its rows into an engine-owned [`Rows`] buffer that stays
//! valid until it is released: by `stickshift_world_query_release`, when the
//! hook or callback that took it returns, or, taken between engine calls, at
//! the next tick. At most [`STICKSHIFT_MAX_LIVE_QUERIES`] are live at once.

use std::cell::RefCell;
use std::ffi::{c_void, CString};

use stickshift_ecs::dynamic::{
    self, Access, Codec, DynError, Key, RegisterError, Registry, Rows, Term, Value,
};
use stickshift_ecs::hecs::{Entity, World};
use stickshift_ecs::{DynamicTag, RenderComponent, RenderShape, StaticTag, TransformComponent};
use stickshift_math::Transform;

use crate::core::{input, output, utf8, write_sized, Error};
use crate::engine::{engine_call, scene_call, SceneAccess, StickShiftEngine};
use crate::types::*;

const _: () = {
    assert!(STICKSHIFT_MAX_GAME_COMPONENTS as usize == dynamic::MAX_GAME_COMPONENTS);
    assert!(STICKSHIFT_MAX_COMPONENT_SIZE == dynamic::MAX_COMPONENT_SIZE);
    assert!(STICKSHIFT_MAX_BUNDLE as usize == dynamic::MAX_BUNDLE);
    assert!(STICKSHIFT_MAX_QUERY_TERMS as usize == dynamic::MAX_QUERY_TERMS);
    // Query rows hand hecs' entity bits out as `StickShiftEntity`s.
    assert!(std::mem::size_of::<StickShiftEntity>() == std::mem::size_of::<u64>());
    assert!(std::mem::align_of::<StickShiftEntity>() == std::mem::align_of::<u64>());
};

/// The engine components, registered first in every engine, in id order.
const ENGINE_COMPONENTS: u32 = 4;

/// The engine components' names, in id order.
const ENGINE_NAMES: [&str; ENGINE_COMPONENTS as usize] = [
    "stickshift_ecs::TransformComponent",
    "stickshift_ecs::RenderComponent",
    "stickshift_ecs::StaticTag",
    "stickshift_ecs::DynamicTag",
];

// ---------------------------------------------------------------------------
// Engine components' byte forms
// ---------------------------------------------------------------------------

/// Write a frozen C value type (plain `f32`/`u32` fields, no padding) into
/// exactly its bytes.
fn put<T: Copy>(value: T, out: &mut [u8]) {
    assert_eq!(out.len(), std::mem::size_of::<T>());
    // SAFETY: `out` is exactly `size_of::<T>()` writable bytes.
    unsafe { out.as_mut_ptr().cast::<T>().write_unaligned(value) }
}

/// Read a frozen C value type from exactly its bytes. Every bit pattern is a
/// valid value of its `f32`/`u32` fields.
fn get<T: Copy>(bytes: &[u8]) -> T {
    assert_eq!(bytes.len(), std::mem::size_of::<T>());
    // SAFETY: `bytes` is exactly `size_of::<T>()` readable bytes, and `T` is
    // one of this module's padding-free C types of `f32`s and `u32`s.
    unsafe { bytes.as_ptr().cast::<T>().read_unaligned() }
}

/// `TransformComponent` as a `StickShiftTransform`, bit for bit.
struct TransformCodec;

impl Codec for TransformCodec {
    type Component = TransformComponent;

    fn encode(value: &TransformComponent, out: &mut [u8]) {
        put(StickShiftTransform::from(value.transform), out);
    }

    fn decode(bytes: &[u8]) -> Option<TransformComponent> {
        Some(TransformComponent::new(Transform::from(get::<
            StickShiftTransform,
        >(bytes))))
    }
}

/// `RenderComponent` as a `StickShiftRenderComponent`: the colour, and which
/// built-in shape its mesh is.
struct RenderCodec;

impl Codec for RenderCodec {
    type Component = RenderComponent;

    fn encode(value: &RenderComponent, out: &mut [u8]) {
        put(
            StickShiftRenderComponent {
                color: value.color,
                shape: shape_of(value),
            },
            out,
        );
    }

    fn decode(bytes: &[u8]) -> Option<RenderComponent> {
        let value = get::<StickShiftRenderComponent>(bytes);
        match value.shape {
            STICKSHIFT_RENDER_SHAPE_CUBE => Some(RenderComponent::cube(value.color)),
            STICKSHIFT_RENDER_SHAPE_SPHERE => Some(RenderComponent::sphere(value.color)),
            // A mesh is made in Rust; C cannot write one.
            _ => None,
        }
    }
}

/// Which built-in shape a render component's mesh is: the one
/// `RenderComponent::cube` or `::sphere` builds for its colour, bit for bit,
/// or a custom mesh.
fn shape_of(value: &RenderComponent) -> StickShiftRenderShape {
    let RenderShape::Mesh { vertices, .. } = &value.shape else {
        return STICKSHIFT_RENDER_SHAPE_MESH;
    };
    let built = match vertices.len() {
        24 => (RenderShape::cube(value.color), STICKSHIFT_RENDER_SHAPE_CUBE),
        441 => (
            RenderComponent::sphere(value.color).shape,
            STICKSHIFT_RENDER_SHAPE_SPHERE,
        ),
        _ => return STICKSHIFT_RENDER_SHAPE_MESH,
    };
    if same_mesh(&value.shape, &built.0) {
        built.1
    } else {
        STICKSHIFT_RENDER_SHAPE_MESH
    }
}

/// Bitwise equality of two meshes (so NaN colours still match themselves).
fn same_mesh(a: &RenderShape, b: &RenderShape) -> bool {
    match (a, b) {
        (
            RenderShape::Mesh {
                vertices: va,
                indices: ia,
            },
            RenderShape::Mesh {
                vertices: vb,
                indices: ib,
            },
        ) => {
            ia == ib
                && va.len() == vb.len()
                && va
                    .iter()
                    .zip(vb)
                    .all(|(x, y)| x.iter().zip(y).all(|(p, q)| p.to_bits() == q.to_bits()))
        }
        _ => false,
    }
}

struct StaticCodec;

impl Codec for StaticCodec {
    type Component = StaticTag;

    fn encode(_: &StaticTag, _: &mut [u8]) {}

    fn decode(_: &[u8]) -> Option<StaticTag> {
        Some(StaticTag)
    }
}

struct DynamicCodec;

impl Codec for DynamicCodec {
    type Component = DynamicTag;

    fn encode(_: &DynamicTag, _: &mut [u8]) {}

    fn decode(_: &[u8]) -> Option<DynamicTag> {
        Some(DynamicTag)
    }
}

// ---------------------------------------------------------------------------
// The engine's ECS state
// ---------------------------------------------------------------------------

/// An engine's component registry and query snapshots. Kept outside the
/// engine's state, so every scope reaches them; no borrow of either is held
/// while a C callback runs.
pub(crate) struct Ecs {
    components: RefCell<Components>,
    queries: RefCell<Queries>,
}

struct Components {
    registry: Registry,
    /// The names as C strings, by registration index.
    names: Vec<CString>,
}

#[derive(Default)]
struct Queries {
    /// Row buffers, kept for reuse once released.
    snapshots: Vec<Snapshot>,
    /// The last token issued.
    token: u64,
}

struct Snapshot {
    rows: Rows,
    /// While live: its token, and the scope depth it was taken at.
    live: Option<(u64, usize)>,
}

impl Ecs {
    pub(crate) fn new() -> Self {
        let mut registry = Registry::new();
        let transform = std::mem::size_of::<StickShiftTransform>() as u32;
        let render = std::mem::size_of::<StickShiftRenderComponent>() as u32;
        let keys = [
            registry.register_native::<TransformCodec>(ENGINE_NAMES[0], transform, 4),
            registry.register_native::<RenderCodec>(ENGINE_NAMES[1], render, 4),
            registry.register_native::<StaticCodec>(ENGINE_NAMES[2], 0, 1),
            registry.register_native::<DynamicCodec>(ENGINE_NAMES[3], 0, 1),
        ];
        for (index, key) in keys.into_iter().enumerate() {
            let key = key.expect("the engine components register in a new registry");
            debug_assert_eq!(key.index() as usize, index);
        }
        let names = ENGINE_NAMES
            .iter()
            .map(|name| CString::new(*name).expect("no NUL in an engine component name"))
            .collect();
        Self {
            components: RefCell::new(Components { registry, names }),
            queries: RefCell::new(Queries::default()),
        }
    }

    /// Release the snapshots taken at scope depth `depth` or deeper: a scope
    /// that ended, or (0) every snapshot, at a tick.
    pub(crate) fn release_from(&self, depth: usize) {
        for snapshot in &mut self.queries.borrow_mut().snapshots {
            if snapshot.live.is_some_and(|(_, d)| d >= depth) {
                snapshot.live = None;
            }
        }
    }
}

impl Components {
    /// The registry key of a C component id.
    fn key(&self, component: StickShiftComponent) -> Result<Key, Error> {
        let index = match component.id {
            id @ 1..=ENGINE_COMPONENTS => Some(id - 1),
            id if id >= STICKSHIFT_FIRST_GAME_COMPONENT => {
                (id - STICKSHIFT_FIRST_GAME_COMPONENT).checked_add(ENGINE_COMPONENTS)
            }
            _ => None,
        };
        index.and_then(|i| self.registry.key(i)).ok_or_else(|| {
            Error::invalid_argument(format!(
                "component {} is not registered with this engine",
                component.id
            ))
        })
    }

    /// The registered size of `key`.
    fn size(&self, key: Key) -> u32 {
        self.registry.info(key).map_or(0, |info| info.size)
    }

    /// A C bundle as registry values, its sizes checked before any data is
    /// read.
    ///
    /// # Safety
    /// `values` must be NULL or valid for `count` values, each `data` valid for
    /// its `size` bytes.
    unsafe fn bundle<'a>(
        &self,
        values: *const StickShiftComponentValue,
        count: u32,
    ) -> Result<Vec<Value<'a>>, Error> {
        if count as usize > dynamic::MAX_BUNDLE {
            return Err(Error::invalid_argument(format!(
                "count {count} is above STICKSHIFT_MAX_BUNDLE ({})",
                dynamic::MAX_BUNDLE
            )));
        }
        if count == 0 {
            return Ok(Vec::new());
        }
        if values.is_null() {
            return Err(Error::null("values"));
        }
        std::slice::from_raw_parts(values, count as usize)
            .iter()
            .enumerate()
            .map(|(i, v)| {
                let key = self.key(v.component)?;
                let size = self.size(key);
                if v.size != size {
                    return Err(Error::invalid_argument(format!(
                        "values[{i}]: component {} has {size} bytes, but size is {}",
                        v.component.id, v.size
                    )));
                }
                if size == 0 {
                    return Ok(Value::tag(key));
                }
                if v.data.is_null() {
                    return Err(Error::null(&format!("values[{i}].data")));
                }
                let data = std::slice::from_raw_parts(v.data.cast::<u8>(), size as usize);
                Ok(Value::new(key, data))
            })
            .collect()
    }

    /// C query terms as registry terms.
    ///
    /// # Safety
    /// `terms` must be NULL or valid for `count` terms.
    unsafe fn terms(
        &self,
        terms: *const StickShiftQueryTerm,
        count: u32,
    ) -> Result<Vec<Term>, Error> {
        if count as usize > dynamic::MAX_QUERY_TERMS {
            return Err(Error::invalid_argument(format!(
                "term_count {count} is above STICKSHIFT_MAX_QUERY_TERMS ({})",
                dynamic::MAX_QUERY_TERMS
            )));
        }
        if count == 0 {
            return Err(Error::invalid_argument(
                "a query needs at least one term that is not STICKSHIFT_QUERY_WITHOUT",
            ));
        }
        if terms.is_null() {
            return Err(Error::null("terms"));
        }
        std::slice::from_raw_parts(terms, count as usize)
            .iter()
            .enumerate()
            .map(|(i, t)| {
                let access = match t.access {
                    STICKSHIFT_QUERY_READ => Access::Read,
                    STICKSHIFT_QUERY_WITH => Access::With,
                    STICKSHIFT_QUERY_WITHOUT => Access::Without,
                    STICKSHIFT_QUERY_OPTIONAL => Access::Optional,
                    other => {
                        return Err(Error::invalid_argument(format!(
                            "terms[{i}].access {other} is not a STICKSHIFT_QUERY_* value"
                        )))
                    }
                };
                Ok(Term::new(self.key(t.component)?, access))
            })
            .collect()
    }

    /// Checks a value's size for `get` or `set`: a tag has none to read or
    /// write.
    fn check_value(&self, component: StickShiftComponent, size: u32) -> Result<Key, Error> {
        let key = self.key(component)?;
        let registered = self.size(key);
        if registered == 0 {
            return Err(Error::invalid_argument(format!(
                "component {} is a tag: it has no value",
                component.id
            )));
        }
        if size != registered {
            return Err(Error::invalid_argument(format!(
                "component {} has {registered} bytes, but size is {size}",
                component.id
            )));
        }
        Ok(key)
    }
}

/// The C id of a registry key.
fn id_of(key: Key) -> u32 {
    if key.index() < ENGINE_COMPONENTS {
        key.index() + 1
    } else {
        STICKSHIFT_FIRST_GAME_COMPONENT + (key.index() - ENGINE_COMPONENTS)
    }
}

fn dyn_error(error: DynError) -> Error {
    Error::invalid_argument(match error {
        DynError::Unknown(key) => format!("component {} is not registered", id_of(key)),
        DynError::SizeMismatch {
            component,
            expected,
            actual,
        } => format!(
            "component {} has {expected} bytes, but {actual} were given",
            id_of(component)
        ),
        DynError::Duplicate(key) => format!("component {} appears twice", id_of(key)),
        DynError::BundleTooLarge(n) => format!(
            "{n} components is above STICKSHIFT_MAX_BUNDLE ({})",
            dynamic::MAX_BUNDLE
        ),
        DynError::NoValue(key) => format!("component {} is a tag: it has no value", id_of(key)),
        DynError::Refused(key) => format!(
            "component {} does not accept that value (for STICKSHIFT_COMPONENT_RENDER, the \
             shape must be STICKSHIFT_RENDER_SHAPE_CUBE or _SPHERE)",
            id_of(key)
        ),
        DynError::TooManyTerms(n) => format!(
            "{n} terms is above STICKSHIFT_MAX_QUERY_TERMS ({})",
            dynamic::MAX_QUERY_TERMS
        ),
        DynError::NoRequiredTerm => {
            "a query needs at least one term that is not STICKSHIFT_QUERY_WITHOUT".into()
        }
    })
}

fn register_error(error: RegisterError, name: &str) -> Error {
    match error {
        RegisterError::LimitExceeded => Error::new(
            STICKSHIFT_ERROR_LIMIT_EXCEEDED,
            format!(
                "STICKSHIFT_MAX_GAME_COMPONENTS ({}) game components are already registered",
                dynamic::MAX_GAME_COMPONENTS
            ),
        ),
        RegisterError::Conflict { existing } => Error::invalid_argument(format!(
            "'{name}' is already registered as component {}, with a different size or \
             alignment",
            id_of(existing)
        )),
        RegisterError::EmptyName => Error::invalid_argument("desc.name is empty"),
        other => Error::invalid_argument(format!("desc: {other}")),
    }
}

/// The hecs entity of a C one; `None` for bits no live entity can have.
pub(crate) fn entity_of(entity: StickShiftEntity) -> Option<Entity> {
    Entity::from_bits(entity.bits)
}

fn to_c(entity: Entity) -> StickShiftEntity {
    StickShiftEntity {
        bits: entity.to_bits().get(),
    }
}

/// The world, for a call that changes it: not in the render hook.
pub(crate) fn world_mut<'a>(access: SceneAccess<'a>, what: &str) -> Result<&'a mut World, Error> {
    match access {
        SceneAccess::Scene { render: true, .. } => Err(Error::wrong_state(format!(
            "{what} changes the world, which the render hook may only read"
        ))),
        SceneAccess::Scene { scene, .. } => Ok(scene.world_mut()),
        SceneAccess::Spawn(cx) => Ok(&mut *cx.world),
    }
}

/// The world, for a call that reads it: in any scope.
fn world_ref(access: SceneAccess<'_>) -> &World {
    match access {
        SceneAccess::Scene { scene, .. } => scene.world(),
        SceneAccess::Spawn(cx) => &*cx.world,
    }
}

// ---------------------------------------------------------------------------
// Components
// ---------------------------------------------------------------------------

/// Register a game component, or get the one already registered under the
/// same name with the same size and alignment. In any scope; registering
/// touches no world state. See `StickShiftComponentDesc`.
///
/// # Safety
/// `engine` must be NULL or a live engine; `desc` NULL or valid, with `name`
/// NULL or NUL-terminated; `out` NULL or valid.
#[no_mangle]
pub unsafe extern "C" fn stickshift_component_register(
    engine: *mut StickShiftEngine,
    desc: *const StickShiftComponentDesc,
    out: *mut StickShiftComponent,
) -> StickShiftResult {
    engine_call(engine, |e| {
        let out = output(out, "out")?;
        *out = StickShiftComponent::default();
        let desc = input(desc, "desc")?;
        let name = utf8(desc.name, "desc.name")?;
        let mut components = e.ecs.components.borrow_mut();
        let key = components
            .registry
            .register(name, desc.size, desc.align)
            .map_err(|error| register_error(error, name))?;
        if key.index() as usize == components.names.len() {
            // From a C string, so it has no interior NUL.
            let name = CString::new(name).map_err(|_| Error::invalid_argument("desc.name"))?;
            components.names.push(name);
        }
        *out = StickShiftComponent { id: id_of(key) };
        Ok(STICKSHIFT_OK)
    })
}

/// What a component is: its name (library-owned, valid for the engine's life),
/// size and alignment. Engine components included. In any scope.
///
/// # Safety
/// `engine` must be NULL or a live engine; `out` NULL or valid.
#[no_mangle]
pub unsafe extern "C" fn stickshift_component_get_desc(
    engine: *mut StickShiftEngine,
    component: StickShiftComponent,
    out: *mut StickShiftComponentDesc,
) -> StickShiftResult {
    engine_call(engine, |e| {
        let out = output(out, "out")?;
        let components = e.ecs.components.borrow();
        let key = components.key(component)?;
        let info = components
            .registry
            .info(key)
            .expect("a key the registry issued");
        *out = StickShiftComponentDesc {
            name: components.names[key.index() as usize].as_ptr(),
            size: info.size,
            align: info.align,
        };
        Ok(STICKSHIFT_OK)
    })
}

// ---------------------------------------------------------------------------
// The world
// ---------------------------------------------------------------------------

/// Spawn one entity with the whole bundle (`count` values, in any order) in
/// one hecs spawn: it lands in the archetype a Rust tuple of the same set of
/// components does. Not in the render hook. In a spawn callback, it spawns
/// into the stream pass's world.
///
/// # Safety
/// `engine` must be NULL or a live engine; `values` NULL or valid for `count`
/// values whose `data` is valid for `size` bytes; `out` NULL or valid.
#[no_mangle]
pub unsafe extern "C" fn stickshift_world_spawn(
    engine: *mut StickShiftEngine,
    values: *const StickShiftComponentValue,
    count: u32,
    out: *mut StickShiftEntity,
) -> StickShiftResult {
    scene_call(engine, "stickshift_world_spawn", |e, access| {
        let out = output(out, "out")?;
        *out = StickShiftEntity::default();
        let components = e.ecs.components.borrow();
        let bundle = components.bundle(values, count)?;
        let world = world_mut(access, "stickshift_world_spawn")?;
        let entity = components
            .registry
            .spawn(world, &bundle)
            .map_err(dyn_error)?;
        *out = to_c(entity);
        Ok(STICKSHIFT_OK)
    })
}

/// Despawn an entity: its id is freed with a bumped generation, and its row
/// is swap-removed. A gone entity is `STICKSHIFT_OK` with `*out_existed = 0`
/// (`out_existed` is optional). Not in the render hook.
///
/// # Safety
/// `engine` must be NULL or a live engine; `out_existed` NULL or valid.
#[no_mangle]
pub unsafe extern "C" fn stickshift_world_despawn(
    engine: *mut StickShiftEngine,
    entity: StickShiftEntity,
    out_existed: *mut u8,
) -> StickShiftResult {
    scene_call(engine, "stickshift_world_despawn", |_, access| {
        if let Some(out) = out_existed.as_mut() {
            *out = 0;
        }
        let world = world_mut(access, "stickshift_world_despawn")?;
        let existed = entity_of(entity).is_some_and(|e| world.despawn(e).is_ok());
        if let Some(out) = out_existed.as_mut() {
            *out = u8::from(existed);
        }
        Ok(STICKSHIFT_OK)
    })
}

/// Whether the entity is alive.
///
/// # Safety
/// `engine` must be NULL or a live engine; `out` NULL or valid.
#[no_mangle]
pub unsafe extern "C" fn stickshift_world_contains(
    engine: *mut StickShiftEngine,
    entity: StickShiftEntity,
    out: *mut u8,
) -> StickShiftResult {
    scene_call(engine, "stickshift_world_contains", |_, access| {
        let out = output(out, "out")?;
        *out = u8::from(entity_of(entity).is_some_and(|e| world_ref(access).contains(e)));
        Ok(STICKSHIFT_OK)
    })
}

/// Whether the entity has the component; 0 for a gone entity.
///
/// # Safety
/// `engine` must be NULL or a live engine; `out` NULL or valid.
#[no_mangle]
pub unsafe extern "C" fn stickshift_world_has(
    engine: *mut StickShiftEngine,
    entity: StickShiftEntity,
    component: StickShiftComponent,
    out: *mut u8,
) -> StickShiftResult {
    scene_call(engine, "stickshift_world_has", |e, access| {
        let out = output(out, "out")?;
        *out = 0;
        let components = e.ecs.components.borrow();
        let key = components.key(component)?;
        let has = match entity_of(entity) {
            Some(entity) => components
                .registry
                .has(world_ref(access), entity, key)
                .map_err(dyn_error)?,
            None => false,
        };
        *out = u8::from(has);
        Ok(STICKSHIFT_OK)
    })
}

/// Copy a component's value into `out` (`size` bytes, the registered size).
/// `STICKSHIFT_ABSENT` if the entity is gone or lacks it; then `out` is
/// unchanged. A tag has no value (`STICKSHIFT_ERROR_INVALID_ARGUMENT`).
///
/// # Safety
/// `engine` must be NULL or a live engine; `out` NULL or valid for `size`
/// bytes.
#[no_mangle]
pub unsafe extern "C" fn stickshift_world_get(
    engine: *mut StickShiftEngine,
    entity: StickShiftEntity,
    component: StickShiftComponent,
    out: *mut c_void,
    size: u32,
) -> StickShiftResult {
    scene_call(engine, "stickshift_world_get", |e, access| {
        let components = e.ecs.components.borrow();
        let key = components.check_value(component, size)?;
        if out.is_null() {
            return Err(Error::null("out"));
        }
        let Some(entity) = entity_of(entity) else {
            return Ok(STICKSHIFT_ABSENT);
        };
        let out = std::slice::from_raw_parts_mut(out.cast::<u8>(), size as usize);
        let found = components
            .registry
            .get(world_ref(access), entity, key, out)
            .map_err(dyn_error)?;
        Ok(if found {
            STICKSHIFT_OK
        } else {
            STICKSHIFT_ABSENT
        })
    })
}

/// Overwrite a component's value in place (`size` bytes, the registered
/// size); the entity does not move. `STICKSHIFT_ABSENT` if the entity is gone
/// or lacks it. Not in the render hook.
///
/// # Safety
/// `engine` must be NULL or a live engine; `data` NULL or valid for `size`
/// bytes.
#[no_mangle]
pub unsafe extern "C" fn stickshift_world_set(
    engine: *mut StickShiftEngine,
    entity: StickShiftEntity,
    component: StickShiftComponent,
    data: *const c_void,
    size: u32,
) -> StickShiftResult {
    scene_call(engine, "stickshift_world_set", |e, access| {
        let components = e.ecs.components.borrow();
        let key = components.check_value(component, size)?;
        if data.is_null() {
            return Err(Error::null("data"));
        }
        let world = world_mut(access, "stickshift_world_set")?;
        let Some(entity) = entity_of(entity) else {
            return Ok(STICKSHIFT_ABSENT);
        };
        let data = std::slice::from_raw_parts(data.cast::<u8>(), size as usize);
        let found = components
            .registry
            .set(world, entity, key, data)
            .map_err(dyn_error)?;
        Ok(if found {
            STICKSHIFT_OK
        } else {
            STICKSHIFT_ABSENT
        })
    })
}

/// Add the bundle's components to an entity, replacing values it already has,
/// in one hecs insert: unless its set of components is unchanged, the entity
/// moves to the end of its new archetype, as in Rust. `STICKSHIFT_ABSENT` for
/// a gone entity. Not in the render hook.
///
/// # Safety
/// As `stickshift_world_spawn`.
#[no_mangle]
pub unsafe extern "C" fn stickshift_world_insert(
    engine: *mut StickShiftEngine,
    entity: StickShiftEntity,
    values: *const StickShiftComponentValue,
    count: u32,
) -> StickShiftResult {
    scene_call(engine, "stickshift_world_insert", |e, access| {
        let components = e.ecs.components.borrow();
        let bundle = components.bundle(values, count)?;
        let world = world_mut(access, "stickshift_world_insert")?;
        let Some(entity) = entity_of(entity) else {
            return Ok(STICKSHIFT_ABSENT);
        };
        let found = components
            .registry
            .insert(world, entity, &bundle)
            .map_err(dyn_error)?;
        Ok(if found {
            STICKSHIFT_OK
        } else {
            STICKSHIFT_ABSENT
        })
    })
}

/// Remove one component from an entity (hecs `remove_one`), which moves it as
/// in Rust. `STICKSHIFT_ABSENT` if the entity is gone or lacks it. Not in the
/// render hook.
///
/// # Safety
/// `engine` must be NULL or a live engine.
#[no_mangle]
pub unsafe extern "C" fn stickshift_world_remove(
    engine: *mut StickShiftEngine,
    entity: StickShiftEntity,
    component: StickShiftComponent,
) -> StickShiftResult {
    scene_call(engine, "stickshift_world_remove", |e, access| {
        let components = e.ecs.components.borrow();
        let key = components.key(component)?;
        let world = world_mut(access, "stickshift_world_remove")?;
        let Some(entity) = entity_of(entity) else {
            return Ok(STICKSHIFT_ABSENT);
        };
        let found = components
            .registry
            .remove(world, entity, key)
            .map_err(dyn_error)?;
        Ok(if found {
            STICKSHIFT_OK
        } else {
            STICKSHIFT_ABSENT
        })
    })
}

/// The number of live entities.
///
/// # Safety
/// `engine` must be NULL or a live engine; `out` NULL or valid.
#[no_mangle]
pub unsafe extern "C" fn stickshift_world_len(
    engine: *mut StickShiftEngine,
    out: *mut u32,
) -> StickShiftResult {
    scene_call(engine, "stickshift_world_len", |_, access| {
        *output(out, "out")? = world_ref(access).len();
        Ok(STICKSHIFT_OK)
    })
}

/// Snapshot the entities matching `terms` in the world's query order
/// (archetypes in creation order, then rows in order), with their values
/// copied into engine-owned rows. Nothing stays borrowed, so the caller may
/// change the world while walking them. At least one term must not be
/// `STICKSHIFT_QUERY_WITHOUT`. `STICKSHIFT_ERROR_LIMIT_EXCEEDED` past
/// `STICKSHIFT_MAX_LIVE_QUERIES` live snapshots. Set
/// `out->size = sizeof(StickShiftQueryRows)`.
///
/// # Safety
/// `engine` must be NULL or a live engine; `terms` NULL or valid for
/// `term_count` terms; `out` NULL or valid for its declared size.
#[no_mangle]
pub unsafe extern "C" fn stickshift_world_query(
    engine: *mut StickShiftEngine,
    terms: *const StickShiftQueryTerm,
    term_count: u32,
    out: *mut StickShiftQueryRows,
) -> StickShiftResult {
    scene_call(engine, "stickshift_world_query", |e, access| {
        let depth = e.scope_depth();
        let components = e.ecs.components.borrow();
        let terms = components.terms(terms, term_count)?;
        let mut queries = e.ecs.queries.borrow_mut();
        let queries = &mut *queries;
        let live = queries
            .snapshots
            .iter()
            .filter(|s| s.live.is_some())
            .count();
        if live >= STICKSHIFT_MAX_LIVE_QUERIES as usize {
            return Err(Error::new(
                STICKSHIFT_ERROR_LIMIT_EXCEEDED,
                format!(
                    "{live} query snapshots are live, STICKSHIFT_MAX_LIVE_QUERIES: release one \
                     with stickshift_world_query_release"
                ),
            ));
        }
        let index = match queries.snapshots.iter().position(|s| s.live.is_none()) {
            Some(index) => index,
            None => {
                queries.snapshots.push(Snapshot {
                    rows: Rows::new(),
                    live: None,
                });
                queries.snapshots.len() - 1
            }
        };
        let snapshot = &mut queries.snapshots[index];
        components
            .registry
            .query(world_ref(access), &terms, &mut snapshot.rows)
            .map_err(dyn_error)?;
        let token = queries.token + 1;
        let rows = &snapshot.rows;
        let value = StickShiftQueryRows {
            size: 0,
            count: rows.len() as u32,
            entities: rows.entity_bits().as_ptr().cast::<StickShiftEntity>(),
            data: rows.data().as_ptr(),
            stride: rows.stride(),
            offsets: rows.offsets().as_ptr(),
            present: rows.present().as_ptr(),
            term_count: rows.term_count() as u32,
            reserved: 0,
            token,
        };
        write_sized(out, value, "out")?;
        queries.token = token;
        snapshot.live = Some((token, depth));
        Ok(STICKSHIFT_OK)
    })
}

/// Count the entities `stickshift_world_query` would snapshot, without
/// copying anything.
///
/// # Safety
/// `engine` must be NULL or a live engine; `terms` NULL or valid for
/// `term_count` terms; `out` NULL or valid.
#[no_mangle]
pub unsafe extern "C" fn stickshift_world_query_count(
    engine: *mut StickShiftEngine,
    terms: *const StickShiftQueryTerm,
    term_count: u32,
    out: *mut u32,
) -> StickShiftResult {
    scene_call(engine, "stickshift_world_query_count", |e, access| {
        let out = output(out, "out")?;
        let components = e.ecs.components.borrow();
        let terms = components.terms(terms, term_count)?;
        *out = components
            .registry
            .query_count(world_ref(access), &terms)
            .map_err(dyn_error)?;
        Ok(STICKSHIFT_OK)
    })
}

/// Release a query snapshot; its rows are invalid after this. In any scope.
/// `STICKSHIFT_ERROR_INVALID_ARGUMENT` for a token that is not live (already
/// released, or released when its scope ended).
///
/// # Safety
/// `engine` must be NULL or a live engine.
#[no_mangle]
pub unsafe extern "C" fn stickshift_world_query_release(
    engine: *mut StickShiftEngine,
    token: u64,
) -> StickShiftResult {
    engine_call(engine, |e| {
        let mut queries = e.ecs.queries.borrow_mut();
        let snapshot = queries
            .snapshots
            .iter_mut()
            .find(|s| token != 0 && s.live.is_some_and(|(t, _)| t == token))
            .ok_or_else(|| {
                Error::invalid_argument(format!(
                    "{token} is not the token of a live query snapshot of this engine"
                ))
            })?;
        snapshot.live = None;
        Ok(STICKSHIFT_OK)
    })
}
