// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

//! Components, the world, queries and the scene through the C API, on null
//! contexts: registration, whole-bundle spawns and the per-entity calls, query
//! rows and their snapshot lifetimes, the streaming configuration and pass,
//! the floating origin, and which scopes allow which calls.
//!
//! Hooks and spawn callbacks never assert (a panic cannot unwind out of
//! `extern "C"`): they record what they see, and the tests check it after.

use std::ffi::{c_void, CStr, CString};
use std::ptr::{null, null_mut};

use stickshift::testing;
use stickshift::*;
use stickshift_math::glam::{Mat4, Vec3};
use stickshift_scene::StreamingConfig;

fn last_error() -> String {
    // SAFETY: never NULL; valid until the next error on this thread.
    unsafe { CStr::from_ptr(stickshift_last_error_message()) }
        .to_string_lossy()
        .into_owned()
}

const T: u32 = STICKSHIFT_COMPONENT_TRANSFORM;
const R: u32 = STICKSHIFT_COMPONENT_RENDER;
const S: u32 = STICKSHIFT_COMPONENT_STATIC_TAG;
const D: u32 = STICKSHIFT_COMPONENT_DYNAMIC_TAG;
const OK: StickShiftResult = STICKSHIFT_OK;
const WS: StickShiftResult = STICKSHIFT_ERROR_WRONG_STATE;
const IA: StickShiftResult = STICKSHIFT_ERROR_INVALID_ARGUMENT;

// ---------------------------------------------------------------------------
// A small harness: a null context, an engine, and a game of scripts
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum At {
    Init,
    Update,
    Render,
    Settings,
    Release,
}

type Script = Box<dyn FnMut(At, *mut StickShiftEngine, &mut Vec<String>)>;

struct Probe {
    script: Script,
    seen: Vec<String>,
    engine: *mut StickShiftEngine,
}

unsafe fn run(user: *mut c_void, at: At, e: *mut StickShiftEngine) {
    let p = &mut *user.cast::<Probe>();
    (p.script)(at, e, &mut p.seen);
}

unsafe extern "C" fn init(user: *mut c_void, e: *mut StickShiftEngine) -> StickShiftResult {
    run(user, At::Init, e);
    OK
}

unsafe extern "C" fn update(user: *mut c_void, e: *mut StickShiftEngine, _: f32) {
    run(user, At::Update, e);
}

unsafe extern "C" fn render(user: *mut c_void, e: *mut StickShiftEngine, _: *mut StickShiftFrame) {
    run(user, At::Render, e);
}

unsafe extern "C" fn settings_changed(
    user: *mut c_void,
    e: *mut StickShiftEngine,
    _: *const StickShiftGraphicsSettings,
) {
    run(user, At::Settings, e);
}

unsafe extern "C" fn release(user: *mut c_void) {
    let e = (*user.cast::<Probe>()).engine;
    run(user, At::Release, e);
}

struct Harness {
    ctx: *mut StickShiftContext,
    engine: *mut StickShiftEngine,
    probe: Box<Probe>,
}

impl Harness {
    fn new(script: impl FnMut(At, *mut StickShiftEngine, &mut Vec<String>) + 'static) -> Self {
        let mut probe = Box::new(Probe {
            script: Box::new(script),
            seen: Vec::new(),
            engine: null_mut(),
        });
        let game = StickShiftGame {
            size: std::mem::size_of::<StickShiftGame>() as u32,
            reserved: 0,
            user: std::ptr::from_mut(&mut *probe).cast(),
            init: Some(init),
            update: Some(update),
            render: Some(render),
            graphics_settings_changed: Some(settings_changed),
            release: Some(release),
        };
        let ctx_desc = StickShiftContextDesc {
            api_version: STICKSHIFT_API_VERSION,
            backend: STICKSHIFT_BACKEND_NULL,
            metal_layer: null_mut(),
            width: 640,
            height: 480,
            scale: 1.0,
        };
        let desc = StickShiftEngineDesc {
            api_version: STICKSHIFT_API_VERSION,
            size: std::mem::size_of::<StickShiftEngineDesc>() as u32,
            audio: STICKSHIFT_AUDIO_SILENT,
            pacing: STICKSHIFT_FRAME_PACING_WAIT,
            flags: 0,
            reserved: 0,
            settings_suite: null(),
        };
        let (mut ctx, mut engine) = (null_mut(), null_mut());
        // SAFETY: valid locals; the probe is boxed, so `user` stays valid.
        unsafe {
            assert_eq!(stickshift_context_create(&ctx_desc, &mut ctx), OK);
            assert_eq!(
                stickshift_engine_create(ctx, &desc, &game, &mut engine),
                OK,
                "{}",
                last_error()
            );
        }
        probe.engine = engine;
        Self { ctx, engine, probe }
    }

    /// An engine whose hooks do nothing.
    fn idle() -> Self {
        Self::new(|_, _, _| {})
    }

    fn init(&self) {
        // SAFETY: a live engine.
        assert_eq!(
            unsafe { stickshift_engine_init(self.engine) },
            OK,
            "{}",
            last_error()
        );
    }

    fn tick(&self) {
        let desc = StickShiftTickDesc {
            size: std::mem::size_of::<StickShiftTickDesc>() as u32,
            clock: STICKSHIFT_CLOCK_FIXED_STEP,
            nanoseconds: 0,
            readback: null_mut(),
            readback_len: 0,
        };
        // SAFETY: a live engine.
        assert_eq!(
            unsafe { stickshift_engine_tick(self.engine, &desc, null_mut()) },
            OK,
            "{}",
            last_error()
        );
    }

    fn seen(&self) -> &[String] {
        &self.probe.seen
    }

    /// Destroy the engine now (running the release hook), keeping the probe.
    fn destroy_engine(&mut self) {
        // SAFETY: a live engine, between engine calls.
        assert_eq!(unsafe { stickshift_engine_destroy(self.engine) }, OK);
        self.engine = null_mut();
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        // SAFETY: the engine first, then its context.
        unsafe {
            if !self.engine.is_null() {
                assert_eq!(stickshift_engine_destroy(self.engine), OK);
            }
            assert_eq!(stickshift_context_destroy(self.ctx), OK);
        }
    }
}

// ---------------------------------------------------------------------------
// Call helpers
// ---------------------------------------------------------------------------

fn c(id: u32) -> StickShiftComponent {
    StickShiftComponent { id }
}

fn val<V>(id: u32, v: &V) -> StickShiftComponentValue {
    StickShiftComponentValue {
        component: c(id),
        data: std::ptr::from_ref(v).cast(),
        size: std::mem::size_of::<V>() as u32,
    }
}

fn tag(id: u32) -> StickShiftComponentValue {
    StickShiftComponentValue {
        component: c(id),
        data: null(),
        size: 0,
    }
}

fn xf(x: f32, y: f32, z: f32) -> StickShiftTransform {
    StickShiftTransform {
        position: StickShiftVec3 { x, y, z },
        rotation: StickShiftQuat {
            x: 0.0,
            y: 0.0,
            z: 0.0,
            w: 1.0,
        },
        scale: StickShiftVec3 {
            x: 1.0,
            y: 1.0,
            z: 1.0,
        },
    }
}

fn cube(r: f32) -> StickShiftRenderComponent {
    StickShiftRenderComponent {
        color: [r, 0.5, 0.25],
        shape: STICKSHIFT_RENDER_SHAPE_CUBE,
    }
}

fn term(id: u32, access: StickShiftQueryAccess) -> StickShiftQueryTerm {
    StickShiftQueryTerm {
        component: c(id),
        access,
    }
}

unsafe fn register(
    e: *mut StickShiftEngine,
    name: &str,
    size: u32,
    align: u32,
) -> (StickShiftResult, u32) {
    let name = CString::new(name).expect("no NUL");
    let desc = StickShiftComponentDesc {
        name: name.as_ptr(),
        size,
        align,
    };
    let mut out = StickShiftComponent { id: 99 };
    let r = stickshift_component_register(e, &desc, &mut out);
    (r, out.id)
}

unsafe fn spawn(
    e: *mut StickShiftEngine,
    values: &[StickShiftComponentValue],
) -> (StickShiftResult, StickShiftEntity) {
    let mut out = StickShiftEntity { bits: 7 };
    let r = stickshift_world_spawn(e, values.as_ptr(), values.len() as u32, &mut out);
    (r, out)
}

/// `stickshift_world_get` of a `V`, or the result code.
unsafe fn get<V: Copy>(
    e: *mut StickShiftEngine,
    entity: StickShiftEntity,
    id: u32,
) -> Result<V, StickShiftResult> {
    let mut out = std::mem::MaybeUninit::<V>::zeroed();
    match stickshift_world_get(
        e,
        entity,
        c(id),
        out.as_mut_ptr().cast(),
        std::mem::size_of::<V>() as u32,
    ) {
        OK => Ok(out.assume_init()),
        other => Err(other),
    }
}

unsafe fn set<V>(
    e: *mut StickShiftEngine,
    entity: StickShiftEntity,
    id: u32,
    v: &V,
) -> StickShiftResult {
    stickshift_world_set(
        e,
        entity,
        c(id),
        std::ptr::from_ref(v).cast(),
        std::mem::size_of::<V>() as u32,
    )
}

unsafe fn has(e: *mut StickShiftEngine, entity: StickShiftEntity, id: u32) -> u8 {
    let mut out = 9;
    assert_eq!(stickshift_world_has(e, entity, c(id), &mut out), OK);
    out
}

unsafe fn contains(e: *mut StickShiftEngine, entity: StickShiftEntity) -> u8 {
    let mut out = 9;
    assert_eq!(stickshift_world_contains(e, entity, &mut out), OK);
    out
}

unsafe fn len(e: *mut StickShiftEngine) -> u32 {
    let mut out = 0;
    assert_eq!(stickshift_world_len(e, &mut out), OK);
    out
}

unsafe fn despawn(e: *mut StickShiftEngine, entity: StickShiftEntity) -> u8 {
    let mut existed = 9;
    assert_eq!(stickshift_world_despawn(e, entity, &mut existed), OK);
    existed
}

fn rows_out() -> StickShiftQueryRows {
    // SAFETY: zero is a valid bit pattern for every field.
    let mut rows: StickShiftQueryRows = unsafe { std::mem::zeroed() };
    rows.size = std::mem::size_of::<StickShiftQueryRows>() as u32;
    rows
}

unsafe fn query(
    e: *mut StickShiftEngine,
    terms: &[StickShiftQueryTerm],
) -> (StickShiftResult, StickShiftQueryRows) {
    let mut rows = rows_out();
    let r = stickshift_world_query(e, terms.as_ptr(), terms.len() as u32, &mut rows);
    (r, rows)
}

unsafe fn entities(rows: &StickShiftQueryRows) -> Vec<StickShiftEntity> {
    std::slice::from_raw_parts(rows.entities, rows.count as usize).to_vec()
}

unsafe fn row_value<V: Copy>(rows: &StickShiftQueryRows, row: usize, term: usize) -> V {
    let offset = *rows.offsets.add(term) as usize;
    rows.data
        .add(row * rows.stride + offset)
        .cast::<V>()
        .read_unaligned()
}

unsafe fn present(rows: &StickShiftQueryRows, row: usize, term: usize) -> u8 {
    *rows.present.add(row * rows.term_count as usize + term)
}

unsafe fn query_count(
    e: *mut StickShiftEngine,
    terms: &[StickShiftQueryTerm],
) -> (StickShiftResult, u32) {
    let mut out = 0;
    let r = stickshift_world_query_count(e, terms.as_ptr(), terms.len() as u32, &mut out);
    (r, out)
}

type SpawnFn<'a> = dyn FnMut(*mut StickShiftEngine, i64, StickShiftVec3) + 'a;

unsafe extern "C" fn trampoline(
    user: *mut c_void,
    e: *mut StickShiftEngine,
    slot: i64,
    position: StickShiftVec3,
) {
    (*user.cast::<&mut SpawnFn<'_>>())(e, slot, position);
}

/// `stickshift_scene_stream` with a Rust closure as the spawn callback.
unsafe fn stream(e: *mut StickShiftEngine, focus: Vec3, f: &mut SpawnFn<'_>) -> StickShiftResult {
    let mut f = f;
    stickshift_scene_stream(
        e,
        focus.into(),
        Some(trampoline),
        std::ptr::from_mut(&mut f).cast(),
    )
}

unsafe fn get_config(e: *mut StickShiftEngine) -> (StickShiftResult, StickShiftStreamingConfig) {
    let mut out = StickShiftStreamingConfig::default();
    let r = stickshift_scene_get_config(e, &mut out);
    (r, out)
}

// ---------------------------------------------------------------------------
// Components
// ---------------------------------------------------------------------------

#[test]
fn engine_components_have_fixed_ids_and_byte_forms() {
    let h = Harness::idle();
    let want = [
        (T, "stickshift_ecs::TransformComponent", 40, 4),
        (R, "stickshift_ecs::RenderComponent", 16, 4),
        (S, "stickshift_ecs::StaticTag", 0, 1),
        (D, "stickshift_ecs::DynamicTag", 0, 1),
    ];
    assert_eq!(std::mem::size_of::<StickShiftTransform>(), 40);
    assert_eq!(std::mem::size_of::<StickShiftRenderComponent>(), 16);
    for (id, name, size, align) in want {
        let mut desc = StickShiftComponentDesc {
            name: null(),
            size: 9,
            align: 9,
        };
        // SAFETY: a live engine and valid locals.
        unsafe {
            assert_eq!(
                stickshift_component_get_desc(h.engine, c(id), &mut desc),
                OK
            );
            assert_eq!(CStr::from_ptr(desc.name).to_str(), Ok(name));
        }
        assert_eq!((desc.size, desc.align), (size, align));
    }
    for id in [0, 5, 255, STICKSHIFT_FIRST_GAME_COMPONENT, u32::MAX] {
        let mut desc = StickShiftComponentDesc {
            name: null(),
            size: 0,
            align: 0,
        };
        // SAFETY: as above.
        assert_eq!(
            unsafe { stickshift_component_get_desc(h.engine, c(id), &mut desc) },
            IA,
            "{id}"
        );
    }
}

#[test]
fn registering_is_idempotent_refuses_conflicts_and_stops_at_the_limit() {
    let h = Harness::idle();
    let e = h.engine;
    // SAFETY: a live engine, between engine calls.
    unsafe {
        let first = STICKSHIFT_FIRST_GAME_COMPONENT;
        assert_eq!(register(e, "game.Health", 4, 4), (OK, first));
        assert_eq!(register(e, "game.Health", 4, 4), (OK, first));
        assert_eq!(register(e, "game.Marked", 0, 1), (OK, first + 1));
        for (name, size, align) in [
            ("game.Health", 8, 4),
            ("game.Health", 4, 8),
            ("game.Health", 0, 1),
            ("game.Big", STICKSHIFT_MAX_COMPONENT_SIZE + 1, 4),
            ("game.Odd", 4, 3),
            ("game.Wide", 4, 32),
            ("game.Zero", 4, 0),
            ("", 4, 4),
            // An engine component's name is taken.
            ("stickshift_ecs::TransformComponent", 40, 4),
        ] {
            assert_eq!(
                register(e, name, size, align),
                (IA, 0),
                "{name} {size} {align}"
            );
        }
        let desc = StickShiftComponentDesc {
            name: null(),
            size: 4,
            align: 4,
        };
        let mut out = c(5);
        assert_eq!(stickshift_component_register(e, &desc, &mut out), IA);
        let bad = b"game.\xff\0";
        let desc = StickShiftComponentDesc {
            name: bad.as_ptr().cast(),
            size: 4,
            align: 4,
        };
        assert_eq!(stickshift_component_register(e, &desc, &mut out), IA);

        let mut desc = StickShiftComponentDesc {
            name: null(),
            size: 0,
            align: 0,
        };
        assert_eq!(stickshift_component_get_desc(e, c(first), &mut desc), OK);
        assert_eq!(CStr::from_ptr(desc.name).to_str(), Ok("game.Health"));
        assert_eq!((desc.size, desc.align), (4, 4));

        // Up to STICKSHIFT_MAX_GAME_COMPONENTS, tags and data together.
        for i in 2..STICKSHIFT_MAX_GAME_COMPONENTS {
            let (size, align) = if i.is_multiple_of(2) { (0, 1) } else { (i, 1) };
            assert_eq!(
                register(e, &format!("game.C{i}"), size, align),
                (OK, first + i),
                "{}",
                last_error()
            );
        }
        assert_eq!(
            register(e, "game.OneTooMany", 4, 4),
            (STICKSHIFT_ERROR_LIMIT_EXCEEDED, 0)
        );
        // At the limit, an existing component still resolves.
        assert_eq!(register(e, "game.Health", 4, 4), (OK, first));
        // And the last one works in the world.
        let last = first + STICKSHIFT_MAX_GAME_COMPONENTS - 1;
        let bytes = [7u8; 127];
        let (r, entity) = spawn(e, &[val(last, &bytes)]);
        assert_eq!(r, OK, "{}", last_error());
        assert_eq!(get::<[u8; 127]>(e, entity, last), Ok(bytes));
    }
}

// ---------------------------------------------------------------------------
// The world
// ---------------------------------------------------------------------------

#[test]
fn spawn_takes_a_whole_bundle_and_values_round_trip() {
    let h = Harness::idle();
    let e = h.engine;
    // SAFETY: a live engine, between engine calls.
    unsafe {
        let (_, health) = register(e, "game.Health", 4, 4);
        let mut t = xf(1.0, -0.0, 3.5);
        t.rotation = StickShiftQuat {
            x: 0.0,
            y: 0.38268343,
            z: 0.0,
            w: 0.9238795,
        };
        let (r, a) = spawn(
            e,
            &[tag(S), val(health, &17u32), val(R, &cube(0.75)), val(T, &t)],
        );
        assert_eq!(r, OK, "{}", last_error());
        assert_ne!(a.bits, 0);
        assert_eq!(len(e), 1);
        let got: StickShiftTransform = get(e, a, T).expect("a transform");
        assert_eq!(
            [
                got.position.x,
                got.position.y,
                got.position.z,
                got.rotation.y,
                got.rotation.w
            ]
            .map(f32::to_bits),
            [1.0f32, -0.0, 3.5, 0.38268343, 0.9238795].map(f32::to_bits)
        );
        assert_eq!(get::<StickShiftRenderComponent>(e, a, R), Ok(cube(0.75)));
        assert_eq!(get::<u32>(e, a, health), Ok(17));
        assert_eq!((has(e, a, S), has(e, a, D), contains(e, a)), (1, 0, 1));

        // A sphere reads back as a sphere; a mesh cannot be written.
        let sphere = StickShiftRenderComponent {
            color: [0.1, 0.2, 0.3],
            shape: STICKSHIFT_RENDER_SHAPE_SPHERE,
        };
        let (r, b) = spawn(e, &[val(T, &t), val(R, &sphere)]);
        assert_eq!(r, OK);
        assert_eq!(get::<StickShiftRenderComponent>(e, b, R), Ok(sphere));
        let mesh = StickShiftRenderComponent {
            shape: STICKSHIFT_RENDER_SHAPE_MESH,
            ..sphere
        };
        assert_eq!(spawn(e, &[val(R, &mesh)]).0, IA);
        assert_eq!(set(e, b, R, &mesh), IA);
        assert_eq!(get::<StickShiftRenderComponent>(e, b, R), Ok(sphere));
        // A recoloured cube stays a cube.
        assert_eq!(set(e, b, R, &cube(0.5)), OK);
        assert_eq!(get::<StickShiftRenderComponent>(e, b, R), Ok(cube(0.5)));

        // A bad bundle spawns nothing.
        let n = len(e);
        let big = [0u8; 8];
        let too_many: Vec<_> = (0..33).map(|_| tag(S)).collect();
        let no_data = StickShiftComponentValue {
            component: c(health),
            data: null(),
            size: 4,
        };
        for (why, bundle) in [
            ("duplicate", vec![tag(S), tag(S)]),
            ("unknown", vec![tag(300)]),
            ("reserved", vec![tag(5)]),
            ("size", vec![val(health, &big)]),
            ("tag with a size", vec![val(S, &big)]),
            ("too many", too_many),
            ("no data", vec![no_data]),
        ] {
            let (r, entity) = spawn(e, &bundle);
            assert_eq!((r, entity.bits), (IA, 0), "{why}");
        }
        assert_eq!(len(e), n);
        let mut out = StickShiftEntity::default();
        assert_eq!(stickshift_world_spawn(e, null(), 1, &mut out), IA);
        assert_eq!(stickshift_world_spawn(e, null(), 0, null_mut()), IA);
        // An empty bundle is an entity with no components.
        assert_eq!(stickshift_world_spawn(e, null(), 0, &mut out), OK);
        assert_eq!((contains(e, out), len(e)), (1, n + 1));

        // get and set check the size, and a tag has no value.
        assert_eq!(get::<u64>(e, a, health), Err(IA));
        assert_eq!(get::<u32>(e, a, S), Err(IA));
        assert_eq!(get::<u32>(e, b, health), Err(STICKSHIFT_ABSENT));
        assert_eq!(set(e, b, health, &5u32), STICKSHIFT_ABSENT);
        assert_eq!(set(e, a, health, &5u64), IA);
        assert_eq!(stickshift_world_get(e, a, c(health), null_mut(), 4), IA);
        assert_eq!(set(e, a, health, &5u32), OK);
        assert_eq!(get::<u32>(e, a, health), Ok(5));
    }
}

#[test]
fn insert_remove_and_despawn_move_entities_as_hecs_does() {
    let h = Harness::idle();
    let e = h.engine;
    let order = |terms: &[StickShiftQueryTerm]| {
        // SAFETY: a live engine, between engine calls.
        unsafe {
            let (r, rows) = query(e, terms);
            assert_eq!(r, OK, "{}", last_error());
            let out = entities(&rows);
            assert_eq!(stickshift_world_query_release(e, rows.token), OK);
            out
        }
    };
    // SAFETY: as above.
    unsafe {
        let (_, hit) = register(e, "game.Hit", 0, 1);
        let ids: Vec<_> = (0..3)
            .map(|i| spawn(e, &[val(T, &xf(i as f32, 0.0, 0.0))]).1)
            .collect();
        let (a, b, c3) = (ids[0], ids[1], ids[2]);
        let only_t = [term(T, STICKSHIFT_QUERY_READ)];
        assert_eq!(order(&only_t), [a, b, c3]);

        // One insert moves `a` to a new archetype: `c` is swapped into its row.
        assert_eq!(
            stickshift_world_insert(e, a, [tag(hit), tag(S)].as_ptr(), 2),
            OK
        );
        assert_eq!(order(&only_t), [c3, b, a]);
        // Replacing a value keeps the set, so nothing moves.
        let moved = xf(9.0, 0.0, 0.0);
        assert_eq!(stickshift_world_insert(e, c3, &val(T, &moved), 1), OK);
        assert_eq!(order(&only_t), [c3, b, a]);
        assert_eq!(get::<StickShiftTransform>(e, c3, T), Ok(moved));

        // remove_one moves it again; a second remove is absent.
        assert_eq!(stickshift_world_remove(e, a, c(hit)), OK);
        assert_eq!(stickshift_world_remove(e, a, c(hit)), STICKSHIFT_ABSENT);
        assert_eq!(stickshift_world_remove(e, a, c(300)), IA);
        assert_eq!((has(e, a, hit), has(e, a, S)), (0, 1));
        assert_eq!(order(&[term(S, STICKSHIFT_QUERY_WITH)]), [a]);

        // Despawn: a gone entity reads absent everywhere, and its id comes back
        // last-freed first with a bumped generation.
        assert_eq!(despawn(e, b), 1);
        assert_eq!(despawn(e, b), 0);
        assert_eq!(stickshift_world_despawn(e, b, null_mut()), OK);
        assert_eq!((contains(e, b), has(e, b, T)), (0, 0));
        assert_eq!(get::<StickShiftTransform>(e, b, T), Err(STICKSHIFT_ABSENT));
        assert_eq!(set(e, b, T, &moved), STICKSHIFT_ABSENT);
        assert_eq!(stickshift_world_insert(e, b, &tag(S), 1), STICKSHIFT_ABSENT);
        assert_eq!(stickshift_world_remove(e, b, c(T)), STICKSHIFT_ABSENT);
        assert_eq!(despawn(e, c3), 1);
        let (_, reused) = spawn(e, &[tag(D)]);
        assert_eq!(reused.bits & 0xffff_ffff, c3.bits & 0xffff_ffff);
        assert_eq!(reused.bits >> 32, (c3.bits >> 32) + 1);
        assert_eq!((contains(e, c3), contains(e, reused)), (0, 1));
        // The null entity and bits no entity has are simply absent.
        for gone in [0, 5, u64::MAX] {
            let gone = StickShiftEntity { bits: gone };
            assert_eq!(
                (contains(e, gone), has(e, gone, T), despawn(e, gone)),
                (0, 0, 0)
            );
            assert_eq!(
                get::<StickShiftTransform>(e, gone, T),
                Err(STICKSHIFT_ABSENT)
            );
        }
        assert_eq!(len(e), 2);
    }
}

// ---------------------------------------------------------------------------
// Queries
// ---------------------------------------------------------------------------

#[repr(C, align(16))]
#[derive(Debug, Clone, Copy, PartialEq)]
struct Wide([f32; 4]);

#[test]
fn query_rows_hold_aligned_values_and_presence_flags() {
    let h = Harness::idle();
    let e = h.engine;
    // SAFETY: a live engine, between engine calls.
    unsafe {
        let (_, byte) = register(e, "game.Byte", 1, 1);
        let (_, wide) = register(e, "game.Wide", 16, 16);
        let (_, long) = register(e, "game.Long", 8, 8);
        let (_, marked) = register(e, "game.Marked", 0, 1);
        let w = Wide([1.0, 2.0, 3.0, 4.0]);
        let (_, a) = spawn(e, &[val(byte, &1u8), val(wide, &w), val(long, &11u64)]);
        let (_, b) = spawn(e, &[val(byte, &2u8), val(long, &22u64), tag(marked)]);
        let (_, c3) = spawn(e, &[val(byte, &3u8), val(wide, &w), tag(marked)]);
        let terms = [
            term(byte, STICKSHIFT_QUERY_READ),
            term(wide, STICKSHIFT_QUERY_OPTIONAL),
            term(marked, STICKSHIFT_QUERY_OPTIONAL),
            term(long, STICKSHIFT_QUERY_OPTIONAL),
            term(T, STICKSHIFT_QUERY_WITHOUT),
        ];
        let (r, rows) = query(e, &terms);
        assert_eq!(r, OK, "{}", last_error());
        assert_eq!(
            rows.size as usize,
            std::mem::size_of::<StickShiftQueryRows>()
        );
        assert_eq!((rows.count, rows.term_count, rows.reserved), (3, 5, 0));
        assert_ne!(rows.token, 0);
        assert_eq!(entities(&rows), [a, b, c3]);
        assert_eq!(rows.data as usize % 16, 0);
        let offsets = std::slice::from_raw_parts(rows.offsets, 5).to_vec();
        // byte at 0, wide at 16, long at 32; the tag and the excluded term hold
        // no value; rows are 48 bytes.
        assert_eq!(offsets, [0, 16, u32::MAX, 32, u32::MAX]);
        assert_eq!(rows.stride, 48);
        for (row, (b8, has_wide, has_marked, l)) in
            [(1u8, 1, 0, 11u64), (2, 0, 1, 22), (3, 1, 1, 0)]
                .into_iter()
                .enumerate()
        {
            assert_eq!(row_value::<u8>(&rows, row, 0), b8);
            assert_eq!(present(&rows, row, 0), 1);
            assert_eq!(present(&rows, row, 1), has_wide);
            assert_eq!(present(&rows, row, 2), has_marked);
            assert_eq!(present(&rows, row, 4), 0);
            assert_eq!(row_value::<u64>(&rows, row, 3), l);
            let expected = if has_wide == 1 { w } else { Wide([0.0; 4]) };
            assert_eq!(row_value::<Wide>(&rows, row, 1), expected);
        }
        assert_eq!(query_count(e, &terms), (OK, 3));
        assert_eq!(
            query_count(e, &[term(marked, STICKSHIFT_QUERY_WITH)]),
            (OK, 2)
        );

        // The snapshot borrows nothing: the world may change under it.
        for entity in [a, b, c3] {
            assert_eq!(despawn(e, entity), 1);
        }
        assert_eq!(row_value::<u8>(&rows, 2, 0), 3);
        assert_eq!(query_count(e, &terms), (OK, 0));
        assert_eq!(stickshift_world_query_release(e, rows.token), OK);
        assert_eq!(stickshift_world_query_release(e, rows.token), IA);
        assert_eq!(stickshift_world_query_release(e, 0), IA);

        // Bad queries.
        let mut out = rows_out();
        for (why, terms) in [
            ("no terms", vec![]),
            ("only excluded", vec![term(T, STICKSHIFT_QUERY_WITHOUT)]),
            ("unknown", vec![term(400, STICKSHIFT_QUERY_READ)]),
            ("access", vec![term(T, 4)]),
            ("too many", vec![term(T, STICKSHIFT_QUERY_READ); 17]),
        ] {
            assert_eq!(query(e, &terms).0, IA, "{why}");
            assert_eq!(query_count(e, &terms).0, IA, "{why}");
        }
        assert_eq!(stickshift_world_query(e, null(), 1, &mut out), IA);
        let one = [term(T, STICKSHIFT_QUERY_READ)];
        assert_eq!(stickshift_world_query(e, one.as_ptr(), 1, null_mut()), IA);
        out.size = 8;
        assert_eq!(stickshift_world_query(e, one.as_ptr(), 1, &mut out), IA);
        // 16 terms are allowed.
        assert_eq!(query(e, &[term(T, STICKSHIFT_QUERY_OPTIONAL); 16]).0, OK);
    }
}

/// Take queries until one fails, recording each result.
unsafe fn take_queries(e: *mut StickShiftEngine, n: usize) -> (Vec<u64>, Vec<StickShiftResult>) {
    let mut tokens = Vec::new();
    let mut results = Vec::new();
    for _ in 0..n {
        let (r, rows) = query(e, &[term(T, STICKSHIFT_QUERY_READ)]);
        results.push(r);
        if r == OK {
            tokens.push(rows.token);
        }
    }
    (tokens, results)
}

#[test]
fn sixteen_snapshots_nest_then_the_limit_and_each_ends_with_its_scope() {
    let mut idle_tokens = Vec::new();
    let mut h = Harness::new(|at, e, seen| {
        // SAFETY: inside a hook of a live engine.
        unsafe {
            if at == At::Update && !seen.iter().any(|s| s.starts_with("update")) {
                let (tokens, results) = take_queries(e, 16);
                seen.push(format!("update took {results:?}"));
                let (r, _) = query(e, &[term(T, STICKSHIFT_QUERY_READ)]);
                seen.push(format!("update 17th {r}"));
                seen.push(format!(
                    "update release {}",
                    stickshift_world_query_release(e, tokens[3])
                ));
                // A spawn callback can take the freed one; it ends with the callback.
                let mut inner = Vec::new();
                let r = stream(e, Vec3::ZERO, &mut |e, _, _| {
                    let (tokens, results) = take_queries(e, 2);
                    inner.push(format!("{results:?} {tokens:?}"));
                });
                seen.push(format!("update stream {r} {inner:?}"));
                let (r, _) = query(e, &[term(T, STICKSHIFT_QUERY_READ)]);
                seen.push(format!("update after stream {r}"));
                seen.push(format!("tokens {:?}", &tokens[..]));
            }
            if at == At::Render && !seen.iter().any(|s| s.starts_with("render")) {
                // The update's snapshots ended with it.
                let (_, results) = take_queries(e, 16);
                seen.push(format!("render took {results:?}"));
            }
        }
    });
    let e = h.engine;
    h.init();
    h.tick();
    let seen = h.seen().to_vec();
    let ok16 = format!("{:?}", [OK; 16]);
    let lim = STICKSHIFT_ERROR_LIMIT_EXCEEDED;
    assert_eq!(seen[0], format!("update took {ok16}"));
    assert_eq!(seen[1], format!("update 17th {lim}"));
    assert_eq!(seen[2], "update release 0");
    assert!(
        seen[3].starts_with(&format!("update stream 0 [\"[0, {lim}]")),
        "{}",
        seen[3]
    );
    // The callback's snapshot was released when it returned.
    assert_eq!(seen[4], "update after stream 0");
    assert_eq!(seen[6], format!("render took {ok16}"));
    let tokens: Vec<u64> = seen[5]
        .trim_start_matches("tokens [")
        .trim_end_matches(']')
        .split(", ")
        .map(|t| t.parse().expect("a token"))
        .collect();
    // SAFETY: a live engine, between engine calls.
    unsafe {
        for token in tokens {
            assert_eq!(stickshift_world_query_release(e, token), IA);
        }
        // Between engine calls, snapshots last until the next tick.
        let (tokens, results) = take_queries(e, 17);
        assert_eq!(results[..16], [OK; 16]);
        assert_eq!(results[16], lim);
        idle_tokens.extend(tokens);
        assert_eq!(stickshift_world_query_release(e, idle_tokens[0]), OK);
    }
    h.tick();
    // SAFETY: as above.
    unsafe {
        for token in &idle_tokens[1..] {
            assert_eq!(stickshift_world_query_release(e, *token), IA);
        }
        assert_eq!(take_queries(e, 16).1, [OK; 16]);
    }
    h.destroy_engine();
}

// ---------------------------------------------------------------------------
// The scene
// ---------------------------------------------------------------------------

#[test]
fn the_default_config_is_the_scenes() {
    let mut out = StickShiftStreamingConfig::default();
    // SAFETY: a valid local, then NULL (a no-op).
    unsafe {
        stickshift_streaming_config_default(&mut out);
        stickshift_streaming_config_default(null_mut());
    }
    assert_eq!(StreamingConfig::from(out), StreamingConfig::default());
    let h = Harness::idle();
    // SAFETY: a live engine.
    let (r, config) = unsafe { get_config(h.engine) };
    assert_eq!((r, config), (OK, out));
}

#[test]
fn set_config_refuses_an_axis_or_spacing_change_once_streaming_started() {
    let h = Harness::idle();
    let e = h.engine;
    // SAFETY: a live engine, between engine calls.
    unsafe {
        let (_, base) = get_config(e);
        // Before the first pass, anything goes.
        let sideways = StickShiftStreamingConfig {
            axis: StickShiftVec3 {
                x: 1.0,
                y: 0.0,
                z: 0.0,
            },
            spawn_interval: 4.0,
            ..base
        };
        assert_eq!(stickshift_scene_set_config(e, &sideways), OK);
        assert_eq!(stickshift_scene_set_config(e, &base), OK);
        // The frontier advances even with no callback.
        assert_eq!(
            stickshift_scene_stream(e, Vec3::ZERO.into(), None, null_mut()),
            OK
        );
        for changed in [
            sideways,
            StickShiftStreamingConfig {
                spawn_interval: 3.0,
                ..base
            },
        ] {
            assert_eq!(stickshift_scene_set_config(e, &changed), WS);
            assert!(last_error().contains("axis"), "{}", last_error());
            assert_eq!(get_config(e), (OK, base));
        }
        // The reach may change mid-stream.
        let reach = StickShiftStreamingConfig {
            spawn_ahead: 90.0,
            despawn_behind: 20.0,
            rebase_threshold: 0.0,
            ..base
        };
        assert_eq!(stickshift_scene_set_config(e, &reach), OK);
        assert_eq!(get_config(e), (OK, reach));
        assert_eq!(stickshift_scene_set_config(e, null()), IA);
        // The slots the NULL pass covered are not filled again: the next pass
        // only fills the new reach (60 to 90 ahead, slots 11..=15).
        let mut slots = Vec::new();
        assert_eq!(
            stream(e, Vec3::ZERO, &mut |_, slot, _| slots.push(slot)),
            OK
        );
        assert_eq!(slots, [11, 12, 13, 14, 15]);
    }
}

#[test]
fn stream_spawns_in_the_pass_tracks_and_despawns_behind() {
    let h = Harness::idle();
    let e = h.engine;
    // SAFETY: a live engine, between engine calls.
    unsafe {
        let (_, marker) = register(e, "game.Marker", 8, 8);
        let mut seen = Vec::new();
        let r = stream(e, Vec3::ZERO, &mut |e, slot, p| {
            let (r, road) = spawn(e, &[val(T, &xf(p.x, p.y, p.z)), tag(S)]);
            let (r2, m) = spawn(e, &[val(marker, &slot)]);
            let t1 = stickshift_stream_track(e, road);
            let t2 = stickshift_stream_track(e, m);
            let null = stickshift_stream_track(e, StickShiftEntity::default());
            seen.push((slot, p.z.to_bits(), [r, r2, t1, t2, null]));
        });
        assert_eq!(r, OK, "{}", last_error());
        // Slots 0..=10 along -Z, 6 apart: Rust's positions, signed zero first.
        assert_eq!(seen.len(), 11);
        for (i, (slot, z, results)) in seen.iter().enumerate() {
            assert_eq!(*slot, i as i64);
            assert_eq!(*z, (-(i as f32) * 6.0).to_bits());
            assert_eq!(*results, [OK, OK, OK, OK, IA]);
        }
        assert_eq!(seen[0].1, (-0.0f32).to_bits());
        let mut streamed = 0;
        assert_eq!(stickshift_scene_streamed_count(e, &mut streamed), OK);
        assert_eq!((streamed, len(e)), (22, 22));
        // Moving 30 ahead despawns what fell 12 behind (roads by their transform,
        // markers by their slot) and fills 5 more slots.
        let mut more = 0;
        assert_eq!(
            stream(e, Vec3::new(0.0, 0.0, -30.0), &mut |e, _, p| {
                let (_, road) = spawn(e, &[val(T, &xf(p.x, p.y, p.z))]);
                stickshift_stream_track(e, road);
                more += 1;
            }),
            OK
        );
        assert_eq!(more, 5);
        assert_eq!(stickshift_scene_streamed_count(e, &mut streamed), OK);
        // Slots 0..=2 (z 0, -6, -12, both entities) are behind -18; 3..=10 stay.
        assert_eq!((streamed, len(e)), (16 + 5, 16 + 5));
    }
}

#[test]
fn maybe_rebase_shifts_every_transform_and_reports_the_offset() {
    let h = Harness::idle();
    let e = h.engine;
    // SAFETY: a live engine, between engine calls.
    unsafe {
        let (_, far) = spawn(e, &[val(T, &xf(2.0, 1.0, -1500.0)), tag(D)]);
        let (_, near) = spawn(e, &[val(T, &xf(0.0, 0.0, -10.0))]);
        let mut offset = StickShiftVec3 {
            x: 9.0,
            y: 9.0,
            z: 9.0,
        };
        assert_eq!(
            stickshift_scene_maybe_rebase(e, Vec3::new(0.0, 0.0, -10.0).into(), &mut offset),
            OK
        );
        assert_eq!(Vec3::from(offset), Vec3::ZERO);
        assert_eq!(
            stickshift_scene_maybe_rebase(e, Vec3::new(2.0, 1.0, -1500.0).into(), &mut offset),
            OK
        );
        assert_eq!(Vec3::from(offset), Vec3::new(0.0, 0.0, 1000.0));
        let t: StickShiftTransform = get(e, far, T).expect("a transform");
        assert_eq!(Vec3::from(t.position), Vec3::new(2.0, 1.0, -500.0));
        let t: StickShiftTransform = get(e, near, T).expect("a transform");
        assert_eq!(Vec3::from(t.position), Vec3::new(0.0, 0.0, 990.0));
        assert_eq!(
            stickshift_scene_maybe_rebase(e, Vec3::ZERO.into(), null_mut()),
            IA
        );
    }
}

#[test]
fn a_far_focus_streams_its_last_slot_once_and_a_non_finite_one_is_refused() {
    let h = Harness::idle();
    let e = h.engine;
    // SAFETY: a live engine, between engine calls.
    unsafe {
        let (_, kept) = spawn(e, &[val(T, &xf(0.0, 0.0, -5.0))]);
        for bad in [f32::NEG_INFINITY, f32::INFINITY, f32::NAN] {
            let focus = Vec3::new(0.0, 0.0, bad);
            let mut called = 0;
            assert_eq!(stream(e, focus, &mut |_, _, _| called += 1), IA);
            assert!(last_error().contains("finite"), "{}", last_error());
            let mut offset = StickShiftVec3::default();
            assert_eq!(
                stickshift_scene_maybe_rebase(e, focus.into(), &mut offset),
                IA
            );
            assert_eq!(called, 0);
        }
        let t: StickShiftTransform = get(e, kept, T).expect("a transform");
        assert_eq!(Vec3::from(t.position), Vec3::new(0.0, 0.0, -5.0));
        // So far along that the slot saturates at i64::MAX: spawned once, and
        // later passes neither overflow the frontier nor poison the engine.
        let mut slots = Vec::new();
        for _ in 0..3 {
            let r = stream(e, Vec3::new(0.0, 0.0, -1.0e30), &mut |_, slot, _| {
                slots.push(slot);
            });
            assert_eq!(r, OK, "{}", last_error());
        }
        assert_eq!(slots, [i64::MAX]);
        let mut n = 0;
        assert_eq!(stickshift_world_len(e, &mut n), OK, "{}", last_error());
    }
}

// ---------------------------------------------------------------------------
// Scopes
// ---------------------------------------------------------------------------

/// Every world and scene call (and a few others), by name, with its result.
/// `destroy` is made only inside an engine call, where it is refused.
unsafe fn calls(
    e: *mut StickShiftEngine,
    entity: StickShiftEntity,
    in_call: bool,
) -> Vec<(&'static str, StickShiftResult)> {
    let t = xf(0.0, 0.0, 0.0);
    let mut out_entity = StickShiftEntity::default();
    let mut byte = 0;
    let mut n = 0;
    let mut v = StickShiftVec3::default();
    let mut rows = rows_out();
    let mut config = StickShiftStreamingConfig::default();
    let one = [term(T, STICKSHIFT_QUERY_READ)];
    let (_, base) = get_config(e);
    let mut desc = StickShiftComponentDesc {
        name: null(),
        size: 0,
        align: 0,
    };
    let mut camera = StickShiftCameraState {
        size: std::mem::size_of::<StickShiftCameraState>() as u32,
        ..Default::default()
    };
    let mut state = StickShiftEngineState {
        size: std::mem::size_of::<StickShiftEngineState>() as u32,
        ..Default::default()
    };
    let mut out = vec![
        (
            "spawn",
            stickshift_world_spawn(e, &val(T, &t), 1, &mut out_entity),
        ),
        ("set", set(e, entity, T, &t)),
        ("insert", stickshift_world_insert(e, entity, &tag(S), 1)),
        ("remove", stickshift_world_remove(e, entity, c(S))),
        (
            "get",
            get::<StickShiftTransform>(e, entity, T).err().unwrap_or(OK),
        ),
        ("has", stickshift_world_has(e, entity, c(T), &mut byte)),
        ("contains", stickshift_world_contains(e, entity, &mut byte)),
        ("len", stickshift_world_len(e, &mut n)),
        (
            "query",
            stickshift_world_query(e, one.as_ptr(), 1, &mut rows),
        ),
        (
            "query_count",
            stickshift_world_query_count(e, one.as_ptr(), 1, &mut n),
        ),
        ("register", register(e, "game.Late", 0, 1).0),
        (
            "get_desc",
            stickshift_component_get_desc(e, c(T), &mut desc),
        ),
        ("get_config", stickshift_scene_get_config(e, &mut config)),
        ("set_config", stickshift_scene_set_config(e, &base)),
        ("streamed_count", stickshift_scene_streamed_count(e, &mut n)),
        (
            "stream",
            stickshift_scene_stream(e, Vec3::ZERO.into(), None, null_mut()),
        ),
        (
            "maybe_rebase",
            stickshift_scene_maybe_rebase(e, Vec3::ZERO.into(), &mut v),
        ),
        ("track", stickshift_stream_track(e, entity)),
        ("camera", stickshift_camera_get(e, &mut camera)),
        (
            "set_camera",
            stickshift_camera_set_position(e, Vec3::ONE.into()),
        ),
        ("get_state", stickshift_engine_get_state(e, &mut state)),
        ("request_exit", stickshift_engine_request_exit(e)),
    ];
    if in_call {
        out.push(("destroy", stickshift_engine_destroy(e)));
    }
    out.push((
        "despawn",
        stickshift_world_despawn(e, out_entity, null_mut()),
    ));
    out
}

fn results(seen: &[String], prefix: &str) -> Vec<String> {
    seen.iter()
        .filter_map(|s| s.strip_prefix(prefix))
        .map(str::to_owned)
        .collect()
}

#[test]
fn the_scope_matrix_for_world_and_scene_calls() {
    let mut h = Harness::new(|at, e, seen| {
        // SAFETY: inside a hook or callback of a live engine.
        unsafe {
            let (_, entity) = spawn(e, &[val(T, &xf(0.0, 0.0, 0.0))]);
            let prefix = format!("{at:?}");
            if seen.iter().any(|s| s.starts_with(&format!("{prefix} "))) {
                return;
            }
            let entity = match at {
                // Render may not spawn: use one an update spawned.
                At::Render => {
                    let (_, rows) = query(e, &[term(T, STICKSHIFT_QUERY_READ)]);
                    entities(&rows)[0]
                }
                // World calls are refused in the release hook anyway.
                At::Release => StickShiftEntity { bits: 1 << 32 },
                _ => entity,
            };
            for (name, r) in calls(e, entity, true) {
                seen.push(format!("{prefix} {name} {r}"));
            }
            if at == At::Update {
                let mut inner = Vec::new();
                let r = stream(e, Vec3::new(0.0, 0.0, -600.0), &mut |e, _, _| {
                    if inner.is_empty() {
                        let (_, entity) = spawn(e, &[val(T, &xf(0.0, 0.0, 0.0))]);
                        inner = calls(e, entity, true);
                    }
                });
                seen.push(format!("Pass {r}"));
                for (name, r) in inner {
                    seen.push(format!("Spawn {name} {r}"));
                }
            }
        }
    });
    h.init();
    // A menu choice after init runs the settings hook inside the push.
    let menu = StickShiftHostEvent {
        kind: STICKSHIFT_HOST_MENU,
        // Shadows: Off (section 1, item 1), a change from the default High.
        code: 101,
        ..Default::default()
    };
    let mut requests = 0;
    // SAFETY: a live engine, between engine calls; one event.
    let pushed = unsafe {
        stickshift_engine_push_events(
            h.engine,
            &menu,
            1,
            std::mem::size_of::<StickShiftHostEvent>() as u32,
            &mut requests,
        )
    };
    assert_eq!(pushed, OK, "{}", last_error());
    h.tick();
    let e = h.engine;
    // Between engine calls.
    let mut idle = Vec::new();
    // SAFETY: a live engine, between engine calls.
    unsafe {
        let (_, entity) = spawn(e, &[val(T, &xf(0.0, 0.0, 0.0))]);
        for (name, r) in calls(e, entity, false) {
            idle.push(format!("Idle {name} {r}"));
        }
    }
    h.destroy_engine();
    let mut seen = h.seen().to_vec();
    seen.extend(idle);

    let names = [
        "spawn",
        "set",
        "insert",
        "remove",
        "get",
        "has",
        "contains",
        "len",
        "query",
        "query_count",
        "register",
        "get_desc",
        "get_config",
        "set_config",
        "streamed_count",
        "stream",
        "maybe_rebase",
        "track",
        "camera",
        "set_camera",
        "get_state",
        "request_exit",
        "destroy",
        "despawn",
    ];
    let expect = |scope: &str, refused: &[&str]| {
        let got = results(&seen, &format!("{scope} "));
        let want: Vec<String> = names
            .iter()
            .filter(|n| !(scope == "Idle" && **n == "destroy"))
            .map(|n| {
                let code = if refused.contains(n) { WS } else { OK };
                format!("{n} {code}")
            })
            .collect();
        assert_eq!(got, want, "{scope}");
    };
    // Game hooks and between calls: everything but tracking (and destroy in a hook).
    expect("Init", &["track", "destroy"]);
    expect("Update", &["track", "destroy"]);
    expect("Settings", &["track", "destroy"]);
    expect("Idle", &["track"]);
    // Render: reads only.
    expect(
        "Render",
        &[
            "spawn",
            "set",
            "insert",
            "remove",
            "set_config",
            "stream",
            "maybe_rebase",
            "track",
            "destroy",
            "despawn",
        ],
    );
    // A spawn callback: the world, tracking, registration; nothing else.
    expect(
        "Spawn",
        &[
            "get_config",
            "set_config",
            "streamed_count",
            "stream",
            "maybe_rebase",
            "camera",
            "set_camera",
            "get_state",
            "request_exit",
            "destroy",
        ],
    );
    // The release hook runs outside every hook: only the calls that touch no
    // engine state.
    let release: Vec<&str> = names
        .iter()
        .copied()
        .filter(|n| !["register", "get_desc"].contains(n))
        .collect();
    expect("Release", &release);
    assert!(seen.contains(&"Pass 0".to_owned()), "{seen:?}");
}

#[test]
fn a_spawn_callback_never_pushes_the_camera() {
    let mut h = Harness::new(|at, e, seen| {
        if at != At::Update || !seen.is_empty() {
            return;
        }
        // SAFETY: inside the update hook of a live engine.
        unsafe {
            let mut ctx = null_mut();
            stickshift_engine_get_context(e, &mut ctx);
            let mut before = (Mat4::IDENTITY, Vec3::ZERO);
            testing::context_camera_unpushed(ctx, &mut before);
            // Moving the camera arms the push for the next context call...
            stickshift_camera_set_position(e, Vec3::new(3.0, 4.0, 5.0).into());
            let mut inside = Vec::new();
            stream(e, Vec3::ZERO, &mut |e, _, _| {
                if inside.is_empty() {
                    // ...but a context call in a spawn callback does not push.
                    let (mut w, mut h) = (0, 0);
                    let mut ctx = null_mut();
                    let r1 = stickshift_engine_get_context(e, &mut ctx);
                    let r2 = stickshift_context_get_size(ctx, &mut w, &mut h);
                    let mut now = (Mat4::IDENTITY, Vec3::ZERO);
                    testing::context_camera_unpushed(ctx, &mut now);
                    inside.push(format!("{r1} {r2} {w}x{h} {}", now.1));
                }
            });
            seen.push(format!("before {}", before.1));
            seen.extend(inside);
            // Back in the hook, the next context call pushes it.
            let (mut w, mut hh) = (0, 0);
            stickshift_context_get_size(ctx, &mut w, &mut hh);
            let mut after = (Mat4::IDENTITY, Vec3::ZERO);
            testing::context_camera_unpushed(ctx, &mut after);
            seen.push(format!("after {}", after.1));
        }
    });
    h.init();
    h.tick();
    let seen = h.seen().to_vec();
    let before = seen[0].trim_start_matches("before ").to_owned();
    assert_eq!(seen[1], format!("0 0 640x480 {before}"));
    assert_eq!(seen[2], format!("after {}", Vec3::new(3.0, 4.0, 5.0)));
    h.destroy_engine();
}
