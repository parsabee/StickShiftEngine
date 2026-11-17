// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

//! The probe differential, world and streaming part: one probe game written
//! twice, as a Rust [`Game`] on the engine's [`Loop`] with typed components and
//! spawn closures, and as C callbacks on a null-context engine that reach the
//! same engine only through the C API. Both print, after init and after each of
//! 3,000 frames, a world dump (every entity in query order with its
//! components' bits), a query-order digest taken in the render hook, the
//! world's length, the streamed count and the rebases so far. The lines must be
//! byte-equal.
//!
//! The probe is demo-shaped: a player `{Transform, Render, Dynamic}`, and per
//! streamed slot a road, buildings and guardrails with game data and tags, an
//! obstacle every third slot, a sphere lamp every fifth and a marker entity
//! with no transform every fourth, all spawned with their whole bundle in the
//! spawn callback. Its updates move the player, rebase the origin (the
//! threshold is low enough for several rebases), stream, despawn queried
//! obstacles and markers, insert and remove a tag (which moves entities between
//! archetypes), overwrite values in place, spawn and despawn entities that are
//! not streamed, and change the reach mid-run.
//!
//! C hooks never assert (a panic cannot unwind out of `extern "C"`): they
//! record unexpected results, and the test checks them at the end.

use std::cell::{Cell, RefCell};
use std::ffi::{c_void, CStr, CString};
use std::ptr::{null, null_mut};

use stickshift::*;
use stickshift_core::driver::{drive_frame, Loop};
use stickshift_core::{EngineCtx, Game, FIXED_DT_DURATION};
use stickshift_ecs::hecs::{Entity, With, Without, World};
use stickshift_ecs::{DynamicTag, RenderComponent, RenderShape, StaticTag, TransformComponent};
use stickshift_math::glam::{Quat, Vec3};
use stickshift_math::Transform;
use stickshift_render_api::NullRenderer;
use stickshift_scene::{SpawnCtx, StreamingConfig};

const FRAMES: u32 = 3000;
const SPEED: f32 = 25.0;
const PLAYER_COLOR: [f32; 3] = [0.1, 0.4, 0.9];

// ---------------------------------------------------------------------------
// The probe's content and rules, shared by both sides
// ---------------------------------------------------------------------------

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
struct Obstacle {
    lane: i32,
    speed: f32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
struct BuildingVariant(u32);

#[derive(Debug, Clone, Copy)]
struct GuardrailTag;

#[derive(Debug, Clone, Copy)]
struct Hit;

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
struct SlotMarker(i64);

fn config() -> StreamingConfig {
    StreamingConfig {
        axis: Vec3::new(0.0, 0.0, -1.0),
        spawn_interval: 6.0,
        spawn_ahead: 60.0,
        despawn_behind: 12.0,
        rebase_threshold: 150.0,
    }
}

/// The reach the probe changes to halfway: axis and spacing stay.
fn wider(c: StreamingConfig) -> StreamingConfig {
    StreamingConfig {
        spawn_ahead: 90.0,
        despawn_behind: 18.0,
        ..c
    }
}

fn player_start() -> Transform {
    Transform::from_position(Vec3::new(0.0, 0.5, 0.0))
}

/// One entity a slot spawns.
enum Piece {
    Road(Transform, [f32; 3]),
    Lamp(Transform, [f32; 3]),
    Obstacle(Transform, Obstacle, [f32; 3]),
    Building(Transform, [f32; 3], BuildingVariant),
    Rail(Transform),
    Marker(SlotMarker),
}

fn slot_pieces(slot: i64, p: Vec3) -> Vec<Piece> {
    let at = |offset: Vec3, rotation: Quat, scale: Vec3| Transform {
        position: p + offset,
        rotation,
        scale,
    };
    let mut pieces = vec![Piece::Road(
        at(Vec3::ZERO, Quat::IDENTITY, Vec3::new(8.0, 0.1, 6.0)),
        [0.2, 0.2, 0.2],
    )];
    if slot.rem_euclid(3) == 2 {
        let lane = (slot / 3).rem_euclid(3) as i32;
        pieces.push(Piece::Obstacle(
            at(
                Vec3::new((lane - 1) as f32 * 2.5, 0.5, 0.0),
                Quat::IDENTITY,
                Vec3::ONE,
            ),
            Obstacle { lane, speed: 0.0 },
            [0.9, 0.1, 0.1],
        ));
    }
    for side in [-1.0f32, 1.0] {
        pieces.push(Piece::Building(
            at(
                Vec3::new(side * 12.0, 4.0, 0.0),
                Quat::from_rotation_y(slot as f32 * 0.1),
                Vec3::new(4.0, 8.0, 4.0),
            ),
            [0.5, 0.5, 0.6],
            BuildingVariant(slot.rem_euclid(4) as u32),
        ));
    }
    for side in [-1.0f32, 1.0] {
        pieces.push(Piece::Rail(at(
            Vec3::new(side * 5.0, 0.4, 0.0),
            Quat::IDENTITY,
            Vec3::new(0.2, 0.8, 6.0),
        )));
    }
    if slot.rem_euclid(5) == 0 {
        pieces.push(Piece::Lamp(
            at(Vec3::new(6.0, 3.0, 0.0), Quat::IDENTITY, Vec3::splat(0.5)),
            [1.0, 0.9, 0.6],
        ));
    }
    if slot.rem_euclid(4) == 1 {
        pieces.push(Piece::Marker(SlotMarker(slot)));
    }
    pieces
}

/// The obstacles update `n` despawns: those in one lane just ahead.
fn obstacles_to_despawn(n: u32, obstacles: &[(u64, Vec3, Obstacle)], player_z: f32) -> Vec<u64> {
    if n % 7 != 3 {
        return Vec::new();
    }
    let lane = ((n / 7) % 3) as i32;
    obstacles
        .iter()
        .filter(|(_, p, o)| o.lane == lane && p.z < player_z && p.z > player_z - 40.0)
        .map(|(bits, _, _)| *bits)
        .collect()
}

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;

fn fnv(mut hash: u64, bytes: &[u8]) -> u64 {
    for b in bytes {
        hash ^= u64::from(*b);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash
}

fn digest_row(hash: u64, bits: u64, p: Vec3) -> u64 {
    let hash = fnv(hash, &bits.to_le_bytes());
    [p.x, p.y, p.z]
        .iter()
        .fold(hash, |h, v| fnv(h, &v.to_bits().to_le_bytes()))
}

fn f(v: f32) -> String {
    format!("{:08x}", v.to_bits())
}

/// One entity's dump line.
#[allow(clippy::too_many_arguments)]
fn entity_line(
    bits: u64,
    t: &Transform,
    render: Option<([f32; 3], u32)>,
    stat: bool,
    dynamic: bool,
    obstacle: Option<Obstacle>,
    building: Option<u32>,
    rail: bool,
    hit: bool,
) -> String {
    let mut s = format!(
        "{bits:016x} t {} {} {} | {} {} {} {} | {} {} {}",
        f(t.position.x),
        f(t.position.y),
        f(t.position.z),
        f(t.rotation.x),
        f(t.rotation.y),
        f(t.rotation.z),
        f(t.rotation.w),
        f(t.scale.x),
        f(t.scale.y),
        f(t.scale.z)
    );
    if let Some((c, shape)) = render {
        s += &format!(" r {} {} {} {shape}", f(c[0]), f(c[1]), f(c[2]));
    }
    if stat {
        s += " static";
    }
    if dynamic {
        s += " dynamic";
    }
    if let Some(o) = obstacle {
        s += &format!(" obstacle {} {}", o.lane, f(o.speed));
    }
    if let Some(b) = building {
        s += &format!(" building {b}");
    }
    if rail {
        s += " rail";
    }
    if hit {
        s += " hit";
    }
    s
}

fn frame_line(frame: &str, len: u32, streamed: u32, digest: u64, rebases: u32) -> String {
    format!("{frame} len {len} streamed {streamed} digest {digest:016x} rebases {rebases}")
}

// ---------------------------------------------------------------------------
// The Rust probe
// ---------------------------------------------------------------------------

fn rust_spawn(cx: &mut SpawnCtx<'_>) {
    for piece in slot_pieces(cx.slot, cx.position) {
        let e = match piece {
            Piece::Road(t, c) => cx.world.spawn((
                TransformComponent::new(t),
                RenderComponent::cube(c),
                StaticTag,
            )),
            Piece::Lamp(t, c) => cx.world.spawn((
                TransformComponent::new(t),
                RenderComponent::sphere(c),
                StaticTag,
            )),
            Piece::Obstacle(t, o, c) => cx.world.spawn((
                TransformComponent::new(t),
                o,
                RenderComponent::cube(c),
                StaticTag,
            )),
            Piece::Building(t, c, b) => cx.world.spawn((
                TransformComponent::new(t),
                RenderComponent::cube(c),
                b,
                StaticTag,
            )),
            Piece::Rail(t) => cx.world.spawn((
                TransformComponent::new(t),
                RenderComponent::cube([0.7, 0.7, 0.7]),
                GuardrailTag,
                StaticTag,
            )),
            Piece::Marker(m) => cx.world.spawn((m,)),
        };
        cx.spawned(e);
    }
}

struct RustProbe {
    player: Option<Entity>,
    updates: u32,
    rebases: u32,
    digest: u64,
}

impl RustProbe {
    fn player_transform(&self, world: &World) -> Transform {
        world
            .get::<&TransformComponent>(self.player.expect("spawned in init"))
            .expect("the player is alive")
            .transform
    }
}

impl Game for RustProbe {
    fn init(&mut self, ctx: &mut EngineCtx<'_>) {
        ctx.scene_mut().set_config(config());
        self.player = Some(ctx.world_mut().spawn((
            TransformComponent::new(player_start()),
            RenderComponent::cube(PLAYER_COLOR),
            DynamicTag,
        )));
        ctx.scene_mut().stream(player_start().position, rust_spawn);
    }

    fn update(&mut self, ctx: &mut EngineCtx<'_>, dt: f32) {
        let n = self.updates;
        self.updates += 1;
        let player = self.player.expect("spawned in init");

        let mut t = self.player_transform(ctx.world());
        t.position.z -= SPEED * dt;
        ctx.world_mut()
            .get::<&mut TransformComponent>(player)
            .expect("the player is alive")
            .transform = t;
        if ctx.scene_mut().maybe_rebase(t.position) != Vec3::ZERO {
            self.rebases += 1;
        }
        let pos = self.player_transform(ctx.world()).position;
        if n == FRAMES / 2 {
            let c = wider(ctx.scene().config());
            ctx.scene_mut().set_config(c);
        }
        ctx.scene_mut().stream(pos, rust_spawn);

        // Query, then despawn.
        let obstacles: Vec<_> = ctx
            .world()
            .query::<(&TransformComponent, &Obstacle)>()
            .iter()
            .map(|(e, (t, o))| (e.to_bits().get(), t.transform.position, *o))
            .collect();
        for bits in obstacles_to_despawn(n, &obstacles, pos.z) {
            let _ = ctx
                .world_mut()
                .despawn(Entity::from_bits(bits).expect("a live entity"));
        }
        if n % 11 == 5 {
            let first = ctx
                .world()
                .query::<Without<(&TransformComponent, &BuildingVariant), &Hit>>()
                .iter()
                .map(|(e, (_, b))| (e, *b))
                .next();
            if let Some((e, b)) = first {
                ctx.world_mut()
                    .insert(e, (Hit, BuildingVariant(b.0 + 100)))
                    .expect("alive");
            }
        }
        if n % 13 == 6 {
            let first = ctx.world().query::<&Hit>().iter().map(|(e, _)| e).next();
            if let Some(e) = first {
                ctx.world_mut().remove_one::<Hit>(e).expect("has it");
            }
        }
        if n % 5 == 1 {
            for (_, o) in ctx.world_mut().query_mut::<&mut Obstacle>() {
                o.speed += 1.0;
            }
        }
        if n % 17 == 8 {
            let first = ctx
                .world()
                .query::<Without<&SlotMarker, &TransformComponent>>()
                .iter()
                .map(|(e, _)| e)
                .next();
            if let Some(e) = first {
                let _ = ctx.world_mut().despawn(e);
            }
        }
        if n % 29 == 10 {
            let drone = Transform::from_position(pos + Vec3::new(0.0, 2.0, 0.0));
            ctx.world_mut()
                .spawn((TransformComponent::new(drone), DynamicTag));
        }
        if n.is_multiple_of(31) && n > 0 {
            let first = ctx
                .world()
                .query::<Without<With<&TransformComponent, &DynamicTag>, &RenderComponent>>()
                .iter()
                .map(|(e, _)| e)
                .next();
            if let Some(e) = first {
                let _ = ctx.world_mut().despawn(e);
            }
        }
    }

    fn render(&mut self, ctx: &mut EngineCtx<'_>) {
        self.digest = ctx
            .world()
            .query::<(&TransformComponent, &RenderComponent)>()
            .iter()
            .fold(FNV_OFFSET, |h, (e, (t, _))| {
                digest_row(h, e.to_bits().get(), t.transform.position)
            });
    }
}

fn rust_shape(r: &RenderComponent) -> u32 {
    if r.shape == RenderShape::cube(r.color) {
        STICKSHIFT_RENDER_SHAPE_CUBE
    } else if r.shape == RenderComponent::sphere(r.color).shape {
        STICKSHIFT_RENDER_SHAPE_SPHERE
    } else {
        STICKSHIFT_RENDER_SHAPE_MESH
    }
}

fn rust_dump(world: &World, lines: &mut Vec<String>) {
    type Row<'a> = (
        &'a TransformComponent,
        Option<&'a RenderComponent>,
        Option<&'a StaticTag>,
        Option<&'a DynamicTag>,
        Option<&'a Obstacle>,
        Option<&'a BuildingVariant>,
        Option<&'a GuardrailTag>,
        Option<&'a Hit>,
    );
    for (e, (t, r, s, d, o, b, g, h)) in world.query::<Row<'_>>().iter() {
        lines.push(entity_line(
            e.to_bits().get(),
            &t.transform,
            r.map(|r| (r.color, rust_shape(r))),
            s.is_some(),
            d.is_some(),
            o.copied(),
            b.map(|b| b.0),
            g.is_some(),
            h.is_some(),
        ));
    }
    for (e, m) in world
        .query::<Without<&SlotMarker, &TransformComponent>>()
        .iter()
    {
        lines.push(format!("{:016x} marker {}", e.to_bits().get(), m.0));
    }
}

fn rust_run() -> Vec<String> {
    let mut lp = Loop::new();
    let mut game = RustProbe {
        player: None,
        updates: 0,
        rebases: 0,
        digest: FNV_OFFSET,
    };
    let mut renderer = NullRenderer::new();
    let mut lines = Vec::new();
    lp.init_once(&mut game, &mut renderer);
    let report = |lp: &Loop, game: &RustProbe, frame: &str, lines: &mut Vec<String>| {
        lines.push(frame_line(
            frame,
            lp.scene.world().len(),
            lp.scene.streamed_count() as u32,
            game.digest,
            game.rebases,
        ));
        rust_dump(lp.scene.world(), lines);
    };
    report(&lp, &game, "init", &mut lines);
    for frame in 0..FRAMES {
        assert_eq!(
            drive_frame(&mut lp, &mut game, &mut renderer, FIXED_DT_DURATION),
            1
        );
        report(&lp, &game, &format!("frame {frame}"), &mut lines);
    }
    lines
}

// ---------------------------------------------------------------------------
// The C probe
// ---------------------------------------------------------------------------

const TRANSFORM: StickShiftComponent = StickShiftComponent {
    id: STICKSHIFT_COMPONENT_TRANSFORM,
};
const RENDER: StickShiftComponent = StickShiftComponent {
    id: STICKSHIFT_COMPONENT_RENDER,
};
const STATIC: StickShiftComponent = StickShiftComponent {
    id: STICKSHIFT_COMPONENT_STATIC_TAG,
};
const DYNAMIC: StickShiftComponent = StickShiftComponent {
    id: STICKSHIFT_COMPONENT_DYNAMIC_TAG,
};

#[derive(Default, Clone, Copy)]
struct Ids {
    obstacle: StickShiftComponent,
    building: StickShiftComponent,
    rail: StickShiftComponent,
    hit: StickShiftComponent,
    marker: StickShiftComponent,
}

/// The C probe's state. Hooks and the spawn callback reach it through shared
/// references only, since a spawn callback runs while the update hook that
/// started the pass is still running.
#[derive(Default)]
struct CProbe {
    ids: Cell<Ids>,
    player: Cell<StickShiftEntity>,
    updates: Cell<u32>,
    rebases: Cell<u32>,
    digest: Cell<u64>,
    errors: RefCell<Vec<String>>,
}

impl CProbe {
    fn check(&self, what: &str, result: StickShiftResult) {
        if result != STICKSHIFT_OK {
            self.errors
                .borrow_mut()
                .push(format!("{what}: {result} ({})", last_error()));
        }
    }
}

fn last_error() -> String {
    // SAFETY: never NULL; valid until the next error on this thread.
    unsafe { CStr::from_ptr(stickshift_last_error_message()) }
        .to_string_lossy()
        .into_owned()
}

fn value<T>(component: StickShiftComponent, v: &T) -> StickShiftComponentValue {
    StickShiftComponentValue {
        component,
        data: std::ptr::from_ref(v).cast(),
        size: std::mem::size_of::<T>() as u32,
    }
}

fn tag(component: StickShiftComponent) -> StickShiftComponentValue {
    StickShiftComponentValue {
        component,
        data: null(),
        size: 0,
    }
}

fn term(component: StickShiftComponent, access: StickShiftQueryAccess) -> StickShiftQueryTerm {
    StickShiftQueryTerm { component, access }
}

/// A query's rows, copied out of the snapshot (which is then released).
struct CRows {
    entities: Vec<u64>,
    data: Vec<u8>,
    stride: usize,
    offsets: Vec<u32>,
    present: Vec<u8>,
    terms: usize,
}

impl CRows {
    fn has(&self, row: usize, term: usize) -> bool {
        self.present[row * self.terms + term] != 0
    }

    fn value<T: Copy>(&self, row: usize, term: usize) -> Option<T> {
        if !self.has(row, term) || self.offsets[term] == u32::MAX {
            return None;
        }
        let at = row * self.stride + self.offsets[term] as usize;
        let bytes = &self.data[at..at + std::mem::size_of::<T>()];
        // SAFETY: the row holds a `T` there (the term's registered size).
        Some(unsafe { bytes.as_ptr().cast::<T>().read_unaligned() })
    }

    fn transform(&self, row: usize, term: usize) -> Transform {
        Transform::from(
            self.value::<StickShiftTransform>(row, term)
                .expect("a transform"),
        )
    }
}

unsafe fn c_query(
    e: *mut StickShiftEngine,
    terms: &[StickShiftQueryTerm],
) -> Result<CRows, String> {
    // SAFETY: zero is a valid bit pattern for every field.
    let mut rows: StickShiftQueryRows = std::mem::zeroed();
    rows.size = std::mem::size_of::<StickShiftQueryRows>() as u32;
    let r = stickshift_world_query(e, terms.as_ptr(), terms.len() as u32, &mut rows);
    if r != STICKSHIFT_OK {
        return Err(format!("query: {r} ({})", last_error()));
    }
    let count = rows.count as usize;
    let terms = rows.term_count as usize;
    let copied = CRows {
        entities: std::slice::from_raw_parts(rows.entities, count)
            .iter()
            .map(|e| e.bits)
            .collect(),
        data: std::slice::from_raw_parts(rows.data, count * rows.stride).to_vec(),
        stride: rows.stride,
        offsets: std::slice::from_raw_parts(rows.offsets, terms).to_vec(),
        present: std::slice::from_raw_parts(rows.present, count * terms).to_vec(),
        terms,
    };
    let r = stickshift_world_query_release(e, rows.token);
    if r != STICKSHIFT_OK {
        return Err(format!("release: {r} ({})", last_error()));
    }
    Ok(copied)
}

unsafe fn c_spawn_bundle(
    p: &CProbe,
    e: *mut StickShiftEngine,
    bundle: &[StickShiftComponentValue],
) -> StickShiftEntity {
    let mut entity = StickShiftEntity::default();
    p.check(
        "spawn",
        stickshift_world_spawn(e, bundle.as_ptr(), bundle.len() as u32, &mut entity),
    );
    entity
}

unsafe extern "C" fn c_spawn(
    user: *mut c_void,
    e: *mut StickShiftEngine,
    slot: i64,
    position: StickShiftVec3,
) {
    let p = &*user.cast::<CProbe>();
    let ids = p.ids.get();
    for piece in slot_pieces(slot, position.into()) {
        let entity = match piece {
            Piece::Road(t, color) | Piece::Lamp(t, color) => {
                let shape = if matches!(piece, Piece::Road(..)) {
                    STICKSHIFT_RENDER_SHAPE_CUBE
                } else {
                    STICKSHIFT_RENDER_SHAPE_SPHERE
                };
                let t = StickShiftTransform::from(t);
                let r = StickShiftRenderComponent { color, shape };
                c_spawn_bundle(
                    p,
                    e,
                    &[value(TRANSFORM, &t), value(RENDER, &r), tag(STATIC)],
                )
            }
            Piece::Obstacle(t, o, color) => {
                let t = StickShiftTransform::from(t);
                let r = StickShiftRenderComponent {
                    color,
                    shape: STICKSHIFT_RENDER_SHAPE_CUBE,
                };
                // A different order from the Rust tuple: the set is what counts.
                c_spawn_bundle(
                    p,
                    e,
                    &[
                        tag(STATIC),
                        value(RENDER, &r),
                        value(ids.obstacle, &o),
                        value(TRANSFORM, &t),
                    ],
                )
            }
            Piece::Building(t, color, b) => {
                let t = StickShiftTransform::from(t);
                let r = StickShiftRenderComponent {
                    color,
                    shape: STICKSHIFT_RENDER_SHAPE_CUBE,
                };
                c_spawn_bundle(
                    p,
                    e,
                    &[
                        value(TRANSFORM, &t),
                        value(RENDER, &r),
                        value(ids.building, &b),
                        tag(STATIC),
                    ],
                )
            }
            Piece::Rail(t) => {
                let t = StickShiftTransform::from(t);
                let r = StickShiftRenderComponent {
                    color: [0.7, 0.7, 0.7],
                    shape: STICKSHIFT_RENDER_SHAPE_CUBE,
                };
                c_spawn_bundle(
                    p,
                    e,
                    &[
                        value(TRANSFORM, &t),
                        value(RENDER, &r),
                        tag(ids.rail),
                        tag(STATIC),
                    ],
                )
            }
            Piece::Marker(m) => c_spawn_bundle(p, e, &[value(ids.marker, &m)]),
        };
        p.check("track", stickshift_stream_track(e, entity));
    }
}

unsafe fn register(
    p: &CProbe,
    e: *mut StickShiftEngine,
    name: &str,
    size: usize,
    align: usize,
) -> StickShiftComponent {
    let name = CString::new(name).expect("no NUL");
    let desc = StickShiftComponentDesc {
        name: name.as_ptr(),
        size: size as u32,
        align: align as u32,
    };
    let mut out = StickShiftComponent::default();
    p.check(
        "register",
        stickshift_component_register(e, &desc, &mut out),
    );
    out
}

unsafe fn c_player_transform(p: &CProbe, e: *mut StickShiftEngine) -> Transform {
    let mut t = StickShiftTransform::from(Transform::from_position(Vec3::ZERO));
    p.check(
        "get player",
        stickshift_world_get(
            e,
            p.player.get(),
            TRANSFORM,
            std::ptr::from_mut(&mut t).cast(),
            std::mem::size_of::<StickShiftTransform>() as u32,
        ),
    );
    Transform::from(t)
}

unsafe fn c_stream(p: &CProbe, e: *mut StickShiftEngine, focus: Vec3) {
    p.check(
        "stream",
        stickshift_scene_stream(
            e,
            focus.into(),
            Some(c_spawn),
            std::ptr::from_ref(p).cast_mut().cast(),
        ),
    );
}

unsafe extern "C" fn c_init(user: *mut c_void, e: *mut StickShiftEngine) -> StickShiftResult {
    let p = &*user.cast::<CProbe>();
    // Registered here, except `Hit`, which the first update that needs it
    // registers: when a component is registered never changes the order.
    p.ids.set(Ids {
        obstacle: register(p, e, "probe.Obstacle", 8, 4),
        building: register(p, e, "probe.BuildingVariant", 4, 4),
        rail: register(p, e, "probe.GuardrailTag", 0, 1),
        hit: StickShiftComponent::default(),
        marker: register(p, e, "probe.SlotMarker", 8, 8),
    });
    let c = StickShiftStreamingConfig::from(config());
    p.check("set_config", stickshift_scene_set_config(e, &c));
    let t = StickShiftTransform::from(player_start());
    let r = StickShiftRenderComponent {
        color: PLAYER_COLOR,
        shape: STICKSHIFT_RENDER_SHAPE_CUBE,
    };
    let player = c_spawn_bundle(
        p,
        e,
        &[tag(DYNAMIC), value(TRANSFORM, &t), value(RENDER, &r)],
    );
    p.player.set(player);
    c_stream(p, e, player_start().position);
    STICKSHIFT_OK
}

unsafe extern "C" fn c_update(user: *mut c_void, e: *mut StickShiftEngine, dt: f32) {
    let p = &*user.cast::<CProbe>();
    let n = p.updates.get();
    p.updates.set(n + 1);
    let mut ids = p.ids.get();

    let mut t = c_player_transform(p, e);
    t.position.z -= SPEED * dt;
    let ct = StickShiftTransform::from(t);
    p.check(
        "set player",
        stickshift_world_set(
            e,
            p.player.get(),
            TRANSFORM,
            std::ptr::from_ref(&ct).cast(),
            std::mem::size_of::<StickShiftTransform>() as u32,
        ),
    );
    let mut offset = StickShiftVec3::default();
    p.check(
        "rebase",
        stickshift_scene_maybe_rebase(e, t.position.into(), &mut offset),
    );
    if Vec3::from(offset) != Vec3::ZERO {
        p.rebases.set(p.rebases.get() + 1);
    }
    let pos = c_player_transform(p, e).position;
    if n == FRAMES / 2 {
        let mut c = StickShiftStreamingConfig::default();
        p.check("get_config", stickshift_scene_get_config(e, &mut c));
        let c = StickShiftStreamingConfig::from(wider(StreamingConfig::from(c)));
        p.check("set_config", stickshift_scene_set_config(e, &c));
    }
    c_stream(p, e, pos);

    let query = |terms: &[StickShiftQueryTerm]| match c_query(e, terms) {
        Ok(rows) => rows,
        Err(why) => {
            p.errors.borrow_mut().push(why);
            CRows {
                entities: Vec::new(),
                data: Vec::new(),
                stride: 0,
                offsets: Vec::new(),
                present: Vec::new(),
                terms: terms.len(),
            }
        }
    };

    // Query, then despawn.
    let rows = query(&[
        term(TRANSFORM, STICKSHIFT_QUERY_READ),
        term(ids.obstacle, STICKSHIFT_QUERY_READ),
    ]);
    let obstacles: Vec<_> = (0..rows.entities.len())
        .map(|i| {
            (
                rows.entities[i],
                rows.transform(i, 0).position,
                rows.value::<Obstacle>(i, 1).expect("read"),
            )
        })
        .collect();
    for bits in obstacles_to_despawn(n, &obstacles, pos.z) {
        let mut existed = 0;
        p.check(
            "despawn obstacle",
            stickshift_world_despawn(e, StickShiftEntity { bits }, &mut existed),
        );
    }
    if n % 11 == 5 {
        if ids.hit.id == 0 {
            ids.hit = register(p, e, "probe.Hit", 0, 1);
            p.ids.set(ids);
        }
        let rows = query(&[
            term(TRANSFORM, STICKSHIFT_QUERY_READ),
            term(ids.building, STICKSHIFT_QUERY_READ),
            term(ids.hit, STICKSHIFT_QUERY_WITHOUT),
        ]);
        if !rows.entities.is_empty() {
            let b = rows.value::<BuildingVariant>(0, 1).expect("read");
            let b = BuildingVariant(b.0 + 100);
            let bundle = [tag(ids.hit), value(ids.building, &b)];
            p.check(
                "insert",
                stickshift_world_insert(
                    e,
                    StickShiftEntity {
                        bits: rows.entities[0],
                    },
                    bundle.as_ptr(),
                    2,
                ),
            );
        }
    }
    if n % 13 == 6 && ids.hit.id != 0 {
        let rows = query(&[term(ids.hit, STICKSHIFT_QUERY_WITH)]);
        if let Some(&bits) = rows.entities.first() {
            p.check(
                "remove",
                stickshift_world_remove(e, StickShiftEntity { bits }, ids.hit),
            );
        }
    }
    if n % 5 == 1 {
        let rows = query(&[term(ids.obstacle, STICKSHIFT_QUERY_READ)]);
        for i in 0..rows.entities.len() {
            let mut o = rows.value::<Obstacle>(i, 0).expect("read");
            o.speed += 1.0;
            p.check(
                "set obstacle",
                stickshift_world_set(
                    e,
                    StickShiftEntity {
                        bits: rows.entities[i],
                    },
                    ids.obstacle,
                    std::ptr::from_ref(&o).cast(),
                    8,
                ),
            );
        }
    }
    if n % 17 == 8 {
        let rows = query(&[
            term(ids.marker, STICKSHIFT_QUERY_READ),
            term(TRANSFORM, STICKSHIFT_QUERY_WITHOUT),
        ]);
        if let Some(&bits) = rows.entities.first() {
            p.check(
                "despawn marker",
                stickshift_world_despawn(e, StickShiftEntity { bits }, null_mut()),
            );
        }
    }
    if n % 29 == 10 {
        let drone =
            StickShiftTransform::from(Transform::from_position(pos + Vec3::new(0.0, 2.0, 0.0)));
        c_spawn_bundle(p, e, &[value(TRANSFORM, &drone), tag(DYNAMIC)]);
    }
    if n.is_multiple_of(31) && n > 0 {
        let rows = query(&[
            term(TRANSFORM, STICKSHIFT_QUERY_WITH),
            term(DYNAMIC, STICKSHIFT_QUERY_WITH),
            term(RENDER, STICKSHIFT_QUERY_WITHOUT),
        ]);
        if let Some(&bits) = rows.entities.first() {
            p.check(
                "despawn drone",
                stickshift_world_despawn(e, StickShiftEntity { bits }, null_mut()),
            );
        }
    }
}

unsafe extern "C" fn c_render(
    user: *mut c_void,
    e: *mut StickShiftEngine,
    _: *mut StickShiftFrame,
) {
    let p = &*user.cast::<CProbe>();
    match c_query(
        e,
        &[
            term(TRANSFORM, STICKSHIFT_QUERY_READ),
            term(RENDER, STICKSHIFT_QUERY_WITH),
        ],
    ) {
        Ok(rows) => p
            .digest
            .set((0..rows.entities.len()).fold(FNV_OFFSET, |h, i| {
                digest_row(h, rows.entities[i], rows.transform(i, 0).position)
            })),
        Err(why) => p.errors.borrow_mut().push(format!("render {why}")),
    }
}

unsafe fn c_dump(e: *mut StickShiftEngine, ids: Ids, lines: &mut Vec<String>) {
    let rows = c_query(
        e,
        &[
            term(TRANSFORM, STICKSHIFT_QUERY_READ),
            term(RENDER, STICKSHIFT_QUERY_OPTIONAL),
            term(STATIC, STICKSHIFT_QUERY_OPTIONAL),
            term(DYNAMIC, STICKSHIFT_QUERY_OPTIONAL),
            term(ids.obstacle, STICKSHIFT_QUERY_OPTIONAL),
            term(ids.building, STICKSHIFT_QUERY_OPTIONAL),
            term(ids.rail, STICKSHIFT_QUERY_OPTIONAL),
            // Registered only once an update needed it.
            term(
                if ids.hit.id == 0 { ids.rail } else { ids.hit },
                STICKSHIFT_QUERY_OPTIONAL,
            ),
        ],
    )
    .expect("the dump query");
    for i in 0..rows.entities.len() {
        lines.push(entity_line(
            rows.entities[i],
            &rows.transform(i, 0),
            rows.value::<StickShiftRenderComponent>(i, 1)
                .map(|r| (r.color, r.shape)),
            rows.has(i, 2),
            rows.has(i, 3),
            rows.value::<Obstacle>(i, 4),
            rows.value::<BuildingVariant>(i, 5).map(|b| b.0),
            rows.has(i, 6),
            ids.hit.id != 0 && rows.has(i, 7),
        ));
    }
    let markers = c_query(
        e,
        &[
            term(ids.marker, STICKSHIFT_QUERY_READ),
            term(TRANSFORM, STICKSHIFT_QUERY_WITHOUT),
        ],
    )
    .expect("the marker query");
    for i in 0..markers.entities.len() {
        let m = markers.value::<SlotMarker>(i, 0).expect("read");
        lines.push(format!("{:016x} marker {}", markers.entities[i], m.0));
    }
}

fn c_run() -> Vec<String> {
    let probe = Box::new(CProbe {
        digest: Cell::new(FNV_OFFSET),
        ..CProbe::default()
    });
    let game = StickShiftGame {
        size: std::mem::size_of::<StickShiftGame>() as u32,
        reserved: 0,
        user: std::ptr::from_ref(&*probe).cast_mut().cast(),
        init: Some(c_init),
        update: Some(c_update),
        render: Some(c_render),
        graphics_settings_changed: None,
        release: None,
    };
    let ctx_desc = StickShiftContextDesc {
        api_version: STICKSHIFT_API_VERSION,
        backend: STICKSHIFT_BACKEND_NULL,
        metal_layer: null_mut(),
        width: 1280,
        height: 720,
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
    let tick = StickShiftTickDesc {
        size: std::mem::size_of::<StickShiftTickDesc>() as u32,
        clock: STICKSHIFT_CLOCK_FIXED_STEP,
        nanoseconds: 0,
        readback: null_mut(),
        readback_len: 0,
    };
    let mut lines = Vec::new();
    // SAFETY: valid locals; the probe outlives the engine.
    unsafe {
        let mut ctx = null_mut();
        let mut engine = null_mut();
        assert_eq!(
            stickshift_context_create(&ctx_desc, &mut ctx),
            STICKSHIFT_OK
        );
        assert_eq!(
            stickshift_engine_create(ctx, &desc, &game, &mut engine),
            STICKSHIFT_OK,
            "{}",
            last_error()
        );
        assert_eq!(
            stickshift_engine_init(engine),
            STICKSHIFT_OK,
            "{}",
            last_error()
        );
        let report = |frame: &str, lines: &mut Vec<String>| {
            let (mut len, mut streamed) = (0, 0);
            assert_eq!(stickshift_world_len(engine, &mut len), STICKSHIFT_OK);
            assert_eq!(
                stickshift_scene_streamed_count(engine, &mut streamed),
                STICKSHIFT_OK
            );
            lines.push(frame_line(
                frame,
                len,
                streamed,
                probe.digest.get(),
                probe.rebases.get(),
            ));
            c_dump(engine, probe.ids.get(), lines);
        };
        report("init", &mut lines);
        for frame in 0..FRAMES {
            let mut out = StickShiftTickResult {
                size: std::mem::size_of::<StickShiftTickResult>() as u32,
                ..Default::default()
            };
            assert_eq!(
                stickshift_engine_tick(engine, &tick, &mut out),
                STICKSHIFT_OK,
                "{}",
                last_error()
            );
            assert_eq!((out.steps, out.rendered), (1, 1));
            report(&format!("frame {frame}"), &mut lines);
        }
        assert_eq!(stickshift_engine_destroy(engine), STICKSHIFT_OK);
        assert_eq!(stickshift_context_destroy(ctx), STICKSHIFT_OK);
    }
    let errors = probe.errors.borrow();
    assert!(
        errors.is_empty(),
        "the C probe saw errors: {:?}",
        &errors[..errors.len().min(10)]
    );
    lines
}

#[test]
fn the_c_probe_prints_the_rust_probes_world_frame_for_frame() {
    let rust = rust_run();
    let c = c_run();
    if let Some(i) = (0..rust.len().max(c.len())).find(|&i| rust.get(i) != c.get(i)) {
        let from = i.saturating_sub(3);
        panic!(
            "line {i} differs.\nRust:\n{}\nC:\n{}",
            rust[from..(i + 3).min(rust.len())].join("\n"),
            c[from..(i + 3).min(c.len())].join("\n")
        );
    }
    // The run did what it is meant to cover.
    let last = rust
        .iter()
        .rev()
        .find(|l| l.starts_with("frame "))
        .expect("frame lines");
    let rebases: u32 = last
        .rsplit(' ')
        .next()
        .and_then(|n| n.parse().ok())
        .expect("a rebase count");
    assert!(rebases >= 5, "only {rebases} rebases: {last}");
    for needle in [" hit", " marker ", " r ", " obstacle ", " building 10"] {
        assert!(
            rust.iter().any(|l| l.contains(needle)),
            "no dump line has {needle:?}"
        );
    }
    // A sphere lamp reads back as a sphere.
    assert!(rust.iter().any(|l| l.contains(" 1 static")));
    // Some entity ids were reused, with a bumped generation.
    assert!(rust.iter().any(|l| l.starts_with("00000002")));
}
