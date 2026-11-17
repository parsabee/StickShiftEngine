// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

//! Tests of `stickshift_ecs::dynamic`, chiefly the differential property test: one
//! seeded script of spawns, despawns, inserts, single removes and sets runs on a
//! world of Rust types and on a world of registered components, and the two must
//! agree on entity bits, `len`, every archetype (its component set and its row
//! order), every value and every query's order.

#![cfg(feature = "dynamic")]

use std::any::TypeId;

use stickshift_ecs::dynamic::{
    Access, Codec, DynError, Key, Kind, RegisterError, Registry, Rows, Term, Value, INLINE_PAYLOAD,
    MAX_BUNDLE, MAX_COMPONENT_SIZE, MAX_GAME_COMPONENTS, MAX_QUERY_TERMS, SLOTS,
};
use stickshift_ecs::hecs::{Entity, World};
use stickshift_ecs::{StaticTag, TransformComponent};
use stickshift_math::glam::{Quat, Vec3};
use stickshift_math::Transform;

// ---------------------------------------------------------------------------
// The Rust-typed side: game-like components and the two natives.

#[derive(Clone, Copy, Debug, PartialEq)]
struct TagA;
#[derive(Clone, Copy, Debug, PartialEq)]
struct TagB;
/// 4 bytes.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Small(u32);
/// Above the inline limit: stored boxed.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Large([u8; 40]);
/// Exactly the inline limit.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Edge([u8; INLINE_PAYLOAD]);

/// The byte form of a transform: position, rotation (x, y, z, w), scale, as f32s.
struct TransformCodec;

const TRANSFORM_SIZE: u32 = 40;

impl Codec for TransformCodec {
    type Component = TransformComponent;

    fn encode(value: &TransformComponent, out: &mut [u8]) {
        let t = &value.transform;
        let floats = [
            t.position.x,
            t.position.y,
            t.position.z,
            t.rotation.x,
            t.rotation.y,
            t.rotation.z,
            t.rotation.w,
            t.scale.x,
            t.scale.y,
            t.scale.z,
        ];
        for (chunk, f) in out.chunks_exact_mut(4).zip(floats) {
            chunk.copy_from_slice(&f.to_ne_bytes());
        }
    }

    /// Refuses a NaN position, to exercise refusal.
    fn decode(bytes: &[u8]) -> Option<TransformComponent> {
        let f: Vec<f32> = bytes
            .chunks_exact(4)
            .map(|c| f32::from_ne_bytes(c.try_into().unwrap()))
            .collect();
        let position = Vec3::new(f[0], f[1], f[2]);
        if position.is_nan() {
            return None;
        }
        Some(TransformComponent::new(Transform {
            position,
            rotation: Quat::from_xyzw(f[3], f[4], f[5], f[6]),
            scale: Vec3::new(f[7], f[8], f[9]),
        }))
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

/// Component indices, shared by both sides.
const T: usize = 0; // TransformComponent (native)
const SM: usize = 1; // Small
const LG: usize = 2; // Large
const ED: usize = 3; // Edge
const TA: usize = 4; // TagA
const TB: usize = 5; // TagB
const ST: usize = 6; // StaticTag (native)
const COMPONENTS: usize = 7;
const SIZES: [usize; COMPONENTS] = [TRANSFORM_SIZE as usize, 4, 40, INLINE_PAYLOAD, 0, 0, 0];

fn typed_ids() -> [TypeId; COMPONENTS] {
    [
        TypeId::of::<TransformComponent>(),
        TypeId::of::<Small>(),
        TypeId::of::<Large>(),
        TypeId::of::<Edge>(),
        TypeId::of::<TagA>(),
        TypeId::of::<TagB>(),
        TypeId::of::<StaticTag>(),
    ]
}

/// Registers the seven components, natives interleaved with game ones.
fn registry() -> (Registry, [Key; COMPONENTS]) {
    let mut r = Registry::new();
    let small = r.register("test::Small", 4, 4).unwrap();
    let t = r
        .register_native::<TransformCodec>("Transform", TRANSFORM_SIZE, 4)
        .unwrap();
    let tag_a = r.register("test::TagA", 0, 1).unwrap();
    let large = r.register("test::Large", 40, 1).unwrap();
    let st = r.register_native::<StaticCodec>("StaticTag", 0, 1).unwrap();
    let edge = r.register("test::Edge", INLINE_PAYLOAD as u32, 1).unwrap();
    let tag_b = r.register("test::TagB", 0, 1).unwrap();
    (r, [t, small, large, edge, tag_a, tag_b, st])
}

// ---------------------------------------------------------------------------
// A deterministic generator.

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        // xorshift64*
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
    fn bytes<const N: usize>(&mut self) -> [u8; N] {
        std::array::from_fn(|_| self.next() as u8)
    }
    fn float(&mut self) -> f32 {
        (self.next() % 20_001) as f32 / 100.0 - 100.0
    }
}

/// One value of every valued component, typed and as bytes.
struct Vals {
    t: TransformComponent,
    small: Small,
    large: Large,
    edge: Edge,
}

impl Vals {
    fn random(rng: &mut Rng) -> Self {
        let mut f = || rng.float();
        let t = TransformComponent::new(Transform {
            position: Vec3::new(f(), f(), f()),
            rotation: Quat::from_xyzw(f(), f(), f(), f()),
            scale: Vec3::new(f(), f(), f()),
        });
        Self {
            t,
            small: Small(rng.next() as u32),
            large: Large(rng.bytes()),
            edge: Edge(rng.bytes()),
        }
    }

    fn bytes(&self, c: usize) -> Vec<u8> {
        match c {
            T => {
                let mut out = vec![0; TRANSFORM_SIZE as usize];
                TransformCodec::encode(&self.t, &mut out);
                out
            }
            SM => self.small.0.to_ne_bytes().to_vec(),
            LG => self.large.0.to_vec(),
            ED => self.edge.0.to_vec(),
            _ => Vec::new(),
        }
    }
}

/// The bundle shapes a Rust game spawns, as tuples. Some are the same set in a
/// different tuple order, which must reach the same archetype.
const SHAPES: &[&[usize]] = &[
    &[],
    &[T],
    &[T, TA],
    &[T, SM],
    &[T, SM, TA],
    &[TA, TB],
    &[LG],
    &[T, LG, TB],
    &[SM, ED, TA, TB],
    &[T, SM, LG, ED, TA, TB, ST],
    &[TB, T],
    &[TA, T],
    &[ST, T, ED],
];

fn typed_spawn(w: &mut World, shape: usize, v: &Vals) -> Entity {
    let (t, s, l, e) = (v.t, v.small, v.large, v.edge);
    match shape {
        0 => w.spawn(()),
        1 => w.spawn((t,)),
        2 => w.spawn((t, TagA)),
        3 => w.spawn((t, s)),
        4 => w.spawn((t, s, TagA)),
        5 => w.spawn((TagA, TagB)),
        6 => w.spawn((l,)),
        7 => w.spawn((t, l, TagB)),
        8 => w.spawn((s, e, TagA, TagB)),
        9 => w.spawn((t, s, l, e, TagA, TagB, StaticTag)),
        10 => w.spawn((TagB, t)),
        11 => w.spawn((TagA, t)),
        12 => w.spawn((StaticTag, t, e)),
        _ => unreachable!(),
    }
}

/// `insert_one`; `false` if the entity is gone.
fn typed_insert_one(w: &mut World, e: Entity, c: usize, v: &Vals) -> bool {
    match c {
        T => w.insert_one(e, v.t),
        SM => w.insert_one(e, v.small),
        LG => w.insert_one(e, v.large),
        ED => w.insert_one(e, v.edge),
        TA => w.insert_one(e, TagA),
        TB => w.insert_one(e, TagB),
        ST => w.insert_one(e, StaticTag),
        _ => unreachable!(),
    }
    .is_ok()
}

/// The multi-component inserts.
const PAIRS: &[&[usize]] = &[&[SM, TA], &[LG, T], &[ED, TB, ST]];

fn typed_insert_pair(w: &mut World, e: Entity, pair: usize, v: &Vals) -> bool {
    match pair {
        0 => w.insert(e, (v.small, TagA)),
        1 => w.insert(e, (v.large, v.t)),
        2 => w.insert(e, (v.edge, TagB, StaticTag)),
        _ => unreachable!(),
    }
    .is_ok()
}

fn typed_remove(w: &mut World, e: Entity, c: usize) -> bool {
    match c {
        T => w.remove_one::<TransformComponent>(e).is_ok(),
        SM => w.remove_one::<Small>(e).is_ok(),
        LG => w.remove_one::<Large>(e).is_ok(),
        ED => w.remove_one::<Edge>(e).is_ok(),
        TA => w.remove_one::<TagA>(e).is_ok(),
        TB => w.remove_one::<TagB>(e).is_ok(),
        ST => w.remove_one::<StaticTag>(e).is_ok(),
        _ => unreachable!(),
    }
}

/// `get::<&mut T>` and overwrite; `false` if gone or lacking.
fn typed_set(w: &mut World, e: Entity, c: usize, v: &Vals) -> bool {
    fn put<X: stickshift_ecs::hecs::Component + Copy>(w: &mut World, e: Entity, x: X) -> bool {
        w.get::<&mut X>(e).map(|mut r| *r = x).is_ok()
    }
    match c {
        T => put(w, e, v.t),
        SM => put(w, e, v.small),
        LG => put(w, e, v.large),
        ED => put(w, e, v.edge),
        _ => unreachable!(),
    }
}

/// The typed value's bytes, or `None` if gone or lacking.
fn typed_get(w: &World, e: Entity, c: usize) -> Option<Vec<u8>> {
    match c {
        T => w.get::<&TransformComponent>(e).ok().map(|t| {
            let mut out = vec![0; TRANSFORM_SIZE as usize];
            TransformCodec::encode(&t, &mut out);
            out
        }),
        SM => w.get::<&Small>(e).ok().map(|x| x.0.to_ne_bytes().to_vec()),
        LG => w.get::<&Large>(e).ok().map(|x| x.0.to_vec()),
        ED => w.get::<&Edge>(e).ok().map(|x| x.0.to_vec()),
        _ => unreachable!(),
    }
}

fn typed_has(w: &World, e: Entity, c: usize) -> bool {
    let id = typed_ids()[c];
    w.entity(e)
        .is_ok_and(|r| r.component_types().any(|t| t == id))
}

// ---------------------------------------------------------------------------
// Queries, typed and dynamic, flattened to (bits, per term Option<bytes>).

type Row = (u64, Vec<Option<Vec<u8>>>);

/// A query's terms, as (component index, access).
type Terms = Vec<(usize, Access)>;

fn bits(e: Entity) -> u64 {
    e.to_bits().get()
}

fn enc_t(t: &TransformComponent) -> Vec<u8> {
    let mut out = vec![0; TRANSFORM_SIZE as usize];
    TransformCodec::encode(t, &mut out);
    out
}

/// Each query as typed hecs runs it, beside its dynamic terms.
fn typed_queries(w: &World) -> Vec<(Terms, Vec<Row>)> {
    use Access::*;
    let mut out = Vec::new();

    let rows = w
        .query::<&TransformComponent>()
        .iter()
        .map(|(e, t)| (bits(e), vec![Some(enc_t(t))]))
        .collect();
    out.push((vec![(T, Read)], rows));

    let rows = w
        .query::<(&Small, &TagA)>()
        .iter()
        .map(|(e, (s, _))| (bits(e), vec![Some(s.0.to_ne_bytes().to_vec()), None]))
        .collect();
    out.push((vec![(SM, Read), (TA, Read)], rows));

    let rows = w
        .query::<(&Large, Option<&Edge>)>()
        .without::<&TagB>()
        .iter()
        .map(|(e, (l, ed))| {
            (
                bits(e),
                vec![Some(l.0.to_vec()), ed.map(|x| x.0.to_vec()), None],
            )
        })
        .collect();
    out.push((vec![(LG, Read), (ED, Optional), (TB, Without)], rows));

    let rows = w
        .query::<&Small>()
        .with::<&TagA>()
        .iter()
        .map(|(e, s)| (bits(e), vec![Some(s.0.to_ne_bytes().to_vec()), None]))
        .collect();
    out.push((vec![(SM, Read), (TA, With)], rows));

    let rows = w
        .query::<(Option<&TransformComponent>, &TagB)>()
        .iter()
        .map(|(e, (t, _))| (bits(e), vec![t.map(enc_t), None]))
        .collect();
    out.push((vec![(T, Optional), (TB, Read)], rows));

    let rows = w
        .query::<(&StaticTag, &Edge, &TransformComponent)>()
        .iter()
        .map(|(e, (_, ed, t))| (bits(e), vec![None, Some(ed.0.to_vec()), Some(enc_t(t))]))
        .collect();
    out.push((vec![(ST, Read), (ED, Read), (T, Read)], rows));

    let rows = w
        .query::<Option<&Small>>()
        .iter()
        .map(|(e, s)| (bits(e), vec![s.map(|x| x.0.to_ne_bytes().to_vec())]))
        .collect();
    out.push((vec![(SM, Optional)], rows));

    let rows = w
        .query::<&TransformComponent>()
        .without::<&StaticTag>()
        .without::<&TagA>()
        .iter()
        .map(|(e, t)| (bits(e), vec![Some(enc_t(t)), None, None]))
        .collect();
    out.push((vec![(T, Read), (ST, Without), (TA, Without)], rows));

    out
}

fn dynamic_rows(rows: &Rows) -> Vec<Row> {
    (0..rows.len())
        .map(|r| {
            let values = (0..rows.term_count())
                .map(|t| rows.value(r, t).map(<[u8]>::to_vec))
                .collect();
            (rows.entity_bits()[r], values)
        })
        .collect()
}

// ---------------------------------------------------------------------------
// The comparison.

fn assert_same(typed: &World, dynamic: &World, reg: &Registry, keys: &[Key; COMPONENTS], at: &str) {
    assert_eq!(typed.len(), dynamic.len(), "{at}: len");

    // Every archetype: same index, same component set, same rows in the same order.
    let ids = typed_ids();
    let to_index = |k: Key| keys.iter().position(|&x| x == k).unwrap();
    assert_eq!(
        typed.archetypes().len(),
        dynamic.archetypes().len(),
        "{at}: archetype count"
    );
    for (i, (ta, da)) in typed.archetypes().zip(dynamic.archetypes()).enumerate() {
        let mut tset: Vec<usize> = ta
            .component_types()
            .map(|t| ids.iter().position(|&x| x == t).unwrap())
            .collect();
        let mut dset: Vec<usize> = da
            .component_types()
            .map(|t| to_index(reg.key_of_type(t).unwrap()))
            .collect();
        tset.sort_unstable();
        dset.sort_unstable();
        assert_eq!(tset, dset, "{at}: archetype {i} components");
        assert_eq!(ta.ids(), da.ids(), "{at}: archetype {i} rows");
    }

    // Every live entity: same components and values.
    for r in typed.iter() {
        let e = r.entity();
        assert!(dynamic.contains(e), "{at}: {e:?} live");
        for c in 0..COMPONENTS {
            let has = typed_has(typed, e, c);
            assert_eq!(reg.has(dynamic, e, keys[c]), Ok(has), "{at}: has {c}");
            if SIZES[c] > 0 {
                let mut out = vec![0; SIZES[c]];
                let found = reg.get(dynamic, e, keys[c], &mut out).unwrap();
                assert_eq!(found, has, "{at}: get {c} found");
                if found {
                    assert_eq!(Some(out), typed_get(typed, e, c), "{at}: value {c}");
                }
            }
        }
    }

    // Every query: same entities, same order, same values; and the same count.
    let mut rows = Rows::new();
    for (q, expected) in typed_queries(typed) {
        let terms: Vec<Term> = q.iter().map(|&(c, a)| Term::new(keys[c], a)).collect();
        reg.query(dynamic, &terms, &mut rows).unwrap();
        assert_eq!(dynamic_rows(&rows), expected, "{at}: query {q:?}");
        assert_eq!(
            reg.query_count(dynamic, &terms).unwrap() as usize,
            expected.len(),
            "{at}: count {q:?}"
        );
    }
}

/// Shuffles a bundle's order: the dynamic side passes components in any order.
fn shuffled(rng: &mut Rng, shape: &[usize]) -> Vec<usize> {
    let mut v = shape.to_vec();
    for i in (1..v.len()).rev() {
        v.swap(i, rng.below(i + 1));
    }
    v
}

fn run_script(seed: u64, steps: usize) {
    let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
    let (reg, keys) = registry();
    let mut typed = World::new();
    let mut dynamic = World::new();
    // Every entity ever spawned, stale ones included.
    let mut handles: Vec<Entity> = Vec::new();

    for step in 0..steps {
        let at = format!("seed {seed} step {step}");
        let v = Vals::random(&mut rng);
        let pick = |rng: &mut Rng, handles: &[Entity]| handles[rng.below(handles.len())];
        match rng.below(12) {
            0..=3 => {
                let shape = rng.below(SHAPES.len());
                let te = typed_spawn(&mut typed, shape, &v);
                let order = shuffled(&mut rng, SHAPES[shape]);
                let bytes: Vec<Vec<u8>> = order.iter().map(|&c| v.bytes(c)).collect();
                let values: Vec<Value> = order
                    .iter()
                    .zip(&bytes)
                    .map(|(&c, b)| Value::new(keys[c], b))
                    .collect();
                let de = reg.spawn(&mut dynamic, &values).unwrap();
                assert_eq!(bits(te), bits(de), "{at}: spawn bits");
                handles.push(te);
            }
            4 | 5 if !handles.is_empty() => {
                let e = pick(&mut rng, &handles);
                assert_eq!(
                    typed.despawn(e).is_ok(),
                    dynamic.despawn(e).is_ok(),
                    "{at}: despawn"
                );
            }
            6 | 7 if !handles.is_empty() => {
                let e = pick(&mut rng, &handles);
                let c = rng.below(COMPONENTS);
                let b = v.bytes(c);
                assert_eq!(
                    Ok(typed_insert_one(&mut typed, e, c, &v)),
                    reg.insert(&mut dynamic, e, &[Value::new(keys[c], &b)]),
                    "{at}: insert {c}"
                );
            }
            8 if !handles.is_empty() => {
                let e = pick(&mut rng, &handles);
                let p = rng.below(PAIRS.len());
                let order = shuffled(&mut rng, PAIRS[p]);
                let bytes: Vec<Vec<u8>> = order.iter().map(|&c| v.bytes(c)).collect();
                let values: Vec<Value> = order
                    .iter()
                    .zip(&bytes)
                    .map(|(&c, b)| Value::new(keys[c], b))
                    .collect();
                assert_eq!(
                    Ok(typed_insert_pair(&mut typed, e, p, &v)),
                    reg.insert(&mut dynamic, e, &values),
                    "{at}: insert pair {p}"
                );
            }
            9 | 10 if !handles.is_empty() => {
                let e = pick(&mut rng, &handles);
                let c = rng.below(COMPONENTS);
                assert_eq!(
                    Ok(typed_remove(&mut typed, e, c)),
                    reg.remove(&mut dynamic, e, keys[c]),
                    "{at}: remove {c}"
                );
            }
            11 if !handles.is_empty() => {
                let e = pick(&mut rng, &handles);
                let c = [T, SM, LG, ED][rng.below(4)];
                let b = v.bytes(c);
                assert_eq!(
                    Ok(typed_set(&mut typed, e, c, &v)),
                    reg.set(&mut dynamic, e, keys[c], &b),
                    "{at}: set {c}"
                );
            }
            _ => {}
        }
        if step % 16 == 15 {
            assert_same(&typed, &dynamic, &reg, &keys, &at);
        }
    }
    assert_same(&typed, &dynamic, &reg, &keys, &format!("seed {seed} end"));
}

#[test]
fn differential_typed_and_dynamic_worlds_agree() {
    for seed in 0..48 {
        run_script(seed, 600);
    }
}

// ---------------------------------------------------------------------------
// Targeted cases.

#[test]
fn ids_are_reused_last_freed_first_with_a_bumped_generation() {
    let (reg, keys) = registry();
    let mut typed = World::new();
    let mut dynamic = World::new();
    let one = |w: &mut World| reg.spawn(w, &[Value::tag(keys[TA])]).unwrap();
    let e: Vec<Entity> = (0..4).map(|_| one(&mut dynamic)).collect();
    let t: Vec<Entity> = (0..4).map(|_| typed.spawn((TagA,))).collect();
    for i in [1, 3] {
        dynamic.despawn(e[i]).unwrap();
        typed.despawn(t[i]).unwrap();
    }
    let a = one(&mut dynamic);
    let b = one(&mut dynamic);
    assert_eq!(a.id(), e[3].id(), "the last freed id comes back first");
    assert_eq!(b.id(), e[1].id());
    assert_eq!(bits(a) >> 32, (bits(e[3]) >> 32) + 1, "generation bumped");
    assert_eq!(bits(a), bits(typed.spawn((TagA,))));
    assert_eq!(bits(b), bits(typed.spawn((TagA,))));
}

#[test]
fn a_stale_entity_reads_as_absent_not_as_the_reused_id() {
    let (reg, keys) = registry();
    let mut w = World::new();
    let small = keys[SM];
    let old = reg
        .spawn(&mut w, &[Value::new(small, &1u32.to_ne_bytes())])
        .unwrap();
    w.despawn(old).unwrap();
    let new = reg
        .spawn(&mut w, &[Value::new(small, &2u32.to_ne_bytes())])
        .unwrap();
    assert_eq!(new.id(), old.id(), "the id is reused");
    assert_ne!(new, old);

    let mut out = [0xAA; 4];
    assert_eq!(reg.get(&w, old, small, &mut out), Ok(false));
    assert_eq!(out, [0xAA; 4], "untouched");
    assert_eq!(reg.has(&w, old, small), Ok(false));
    assert_eq!(reg.set(&mut w, old, small, &9u32.to_ne_bytes()), Ok(false));
    assert_eq!(reg.insert(&mut w, old, &[Value::tag(keys[TA])]), Ok(false));
    assert_eq!(reg.remove(&mut w, old, small), Ok(false));
    assert!(!w.contains(old));

    // The live entity on the reused id is untouched by all of that.
    assert_eq!(reg.get(&w, new, small, &mut out), Ok(true));
    assert_eq!(u32::from_ne_bytes(out), 2);
    assert_eq!(reg.has(&w, new, keys[TA]), Ok(false));
}

#[test]
fn payloads_above_the_inline_limit_round_trip() {
    let mut reg = Registry::new();
    let sizes = [1u32, 31, 32, 33, 100, 1000, MAX_COMPONENT_SIZE];
    let keys: Vec<Key> = sizes
        .iter()
        .map(|&s| reg.register(&format!("blob{s}"), s, 1).unwrap())
        .collect();
    let pattern = |s: u32, salt: u8| -> Vec<u8> {
        (0..s).map(|i| (i as u8).wrapping_mul(31) ^ salt).collect()
    };
    let data: Vec<Vec<u8>> = sizes.iter().map(|&s| pattern(s, 1)).collect();
    let values: Vec<Value> = keys
        .iter()
        .zip(&data)
        .map(|(&k, d)| Value::new(k, d))
        .collect();
    let mut w = World::new();
    let e = reg.spawn(&mut w, &values).unwrap();

    for (&k, &s) in keys.iter().zip(&sizes) {
        let mut out = vec![0; s as usize];
        assert_eq!(reg.get(&w, e, k, &mut out), Ok(true));
        assert_eq!(out, pattern(s, 1), "size {s}");
        assert_eq!(reg.set(&mut w, e, k, &pattern(s, 2)), Ok(true));
    }

    // A query copies every value whole, and an insert replaces in place.
    let terms: Vec<Term> = keys.iter().map(|&k| Term::new(k, Access::Read)).collect();
    let mut rows = Rows::new();
    reg.query(&w, &terms, &mut rows).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows.stride(), sizes.iter().sum::<u32>() as usize);
    for (t, &s) in sizes.iter().enumerate() {
        assert_eq!(rows.value(0, t), Some(&pattern(s, 2)[..]), "size {s}");
    }
    let big = keys[keys.len() - 1];
    let replaced = pattern(MAX_COMPONENT_SIZE, 3);
    assert_eq!(
        reg.insert(&mut w, e, &[Value::new(big, &replaced)]),
        Ok(true)
    );
    let mut out = vec![0; MAX_COMPONENT_SIZE as usize];
    assert_eq!(reg.get(&w, e, big, &mut out), Ok(true));
    assert_eq!(out, replaced);
    assert_eq!(w.archetypes().len(), 2, "no move: same set of components");
}

#[test]
fn registration_rules() {
    let mut reg = Registry::new();
    let a = reg.register("pkg.Health", 4, 4).unwrap();
    assert_eq!(reg.register("pkg.Health", 4, 4), Ok(a), "idempotent");
    assert_eq!(
        reg.register("pkg.Health", 8, 4),
        Err(RegisterError::Conflict { existing: a }),
        "same name, different size"
    );
    assert_eq!(
        reg.register("pkg.Health", 4, 2),
        Err(RegisterError::Conflict { existing: a }),
        "same name, different alignment"
    );
    let tag = reg.register("pkg.Flag", 0, 1).unwrap();
    assert_eq!(
        reg.register("pkg.Flag", 4, 1),
        Err(RegisterError::Conflict { existing: tag })
    );
    assert_eq!(reg.register("", 4, 4), Err(RegisterError::EmptyName));
    assert_eq!(
        reg.register("big", MAX_COMPONENT_SIZE + 1, 1),
        Err(RegisterError::TooLarge {
            size: MAX_COMPONENT_SIZE + 1
        })
    );
    for align in [0, 3, 32] {
        assert_eq!(
            reg.register("odd", 4, align),
            Err(RegisterError::BadAlign { align })
        );
    }

    let t = reg
        .register_native::<TransformCodec>("Transform", TRANSFORM_SIZE, 4)
        .unwrap();
    assert_eq!(
        reg.register_native::<TransformCodec>("Transform", TRANSFORM_SIZE, 4),
        Ok(t)
    );
    assert_eq!(
        reg.register_native::<TransformCodec>("Transform2", TRANSFORM_SIZE, 4),
        Err(RegisterError::Conflict { existing: t }),
        "one name per native type"
    );
    assert_eq!(
        reg.register("Transform", TRANSFORM_SIZE, 4),
        Err(RegisterError::Conflict { existing: t }),
        "a game component cannot take a native's name"
    );
    assert_eq!(
        reg.register_native::<StaticCodec>("pkg.Health", 4, 4),
        Err(RegisterError::Conflict { existing: a }),
    );

    let info = reg.info(t).unwrap();
    assert_eq!((info.name, info.size, info.align), ("Transform", 40, 4));
    assert_eq!(info.kind, Kind::Native);
    assert_eq!(info.type_id, TypeId::of::<TransformComponent>());
    assert_eq!(reg.info(tag).unwrap().kind, Kind::Tag);
    assert_eq!(reg.info(a).unwrap().kind, Kind::Data);
    assert_eq!(reg.lookup("pkg.Flag"), Some(tag));
    assert_eq!(reg.key(t.index()), Some(t));
    assert_eq!(reg.key(reg.len() as u32), None);
    assert_eq!(reg.game_count(), 2);
}

#[test]
fn the_limit_is_shared_by_tags_and_data_and_every_slot_works() {
    let mut reg = Registry::new();
    let mut keys = Vec::new();
    for i in 0..MAX_GAME_COMPONENTS {
        let size = if i % 2 == 0 { 0 } else { 2 };
        keys.push(reg.register(&format!("c{i}"), size, 1).unwrap());
    }
    assert_eq!(
        reg.register("one more", 0, 1),
        Err(RegisterError::LimitExceeded)
    );
    assert_eq!(
        reg.register("one more", 2, 1),
        Err(RegisterError::LimitExceeded)
    );
    assert_eq!(
        reg.register("c7", 2, 1),
        Ok(keys[7]),
        "existing names still resolve"
    );
    assert!(
        reg.register_native::<StaticCodec>("StaticTag", 0, 1)
            .is_ok(),
        "natives take no slot"
    );

    // One entity per component: each lands in its own archetype and reads back.
    let mut w = World::new();
    let ents: Vec<Entity> = keys
        .iter()
        .enumerate()
        .map(|(i, &k)| {
            let data = (i as u16).to_ne_bytes();
            let v = if i % 2 == 0 {
                Value::tag(k)
            } else {
                Value::new(k, &data)
            };
            reg.spawn(&mut w, &[v]).unwrap()
        })
        .collect();
    assert_eq!(w.archetypes().len(), 1 + MAX_GAME_COMPONENTS);
    for (i, (&k, &e)) in keys.iter().zip(&ents).enumerate() {
        assert_eq!(reg.has(&w, e, k), Ok(true));
        assert_eq!(reg.query_count(&w, &[Term::new(k, Access::With)]), Ok(1));
        if i % 2 == 1 {
            let mut out = [0; 2];
            assert_eq!(reg.get(&w, e, k, &mut out), Ok(true));
            assert_eq!(u16::from_ne_bytes(out), i as u16);
        }
    }
}

#[test]
fn a_whole_family_of_one_kind_fits() {
    for size in [0u32, 8] {
        let mut reg = Registry::new();
        let keys: Vec<Key> = (0..SLOTS)
            .map(|i| reg.register(&format!("k{i}"), size, 1).unwrap())
            .collect();
        let mut w = World::new();
        let data = vec![5u8; size as usize];
        // Bundles of up to MAX_BUNDLE, so the slots also mix within archetypes.
        for chunk in keys.chunks(MAX_BUNDLE) {
            let values: Vec<Value> = chunk.iter().map(|&k| Value::new(k, &data)).collect();
            reg.spawn(&mut w, &values).unwrap();
        }
        for &k in &keys {
            assert_eq!(reg.query_count(&w, &[Term::new(k, Access::Read)]), Ok(1));
        }
    }
}

#[test]
fn a_refused_bundle_spawns_nothing() {
    let (reg, keys) = registry();
    let (other, _) = {
        let mut r = Registry::new();
        for i in 0..20 {
            r.register(&format!("x{i}"), 0, 1).unwrap();
        }
        let k = r.key(19).unwrap();
        (k, r)
    };
    let mut w = World::new();
    let first = reg.spawn(&mut w, &[]).unwrap();
    w.despawn(first).unwrap();
    let archetypes = w.archetypes().len();

    let four = 4u32.to_ne_bytes();
    let mut nan = vec![0u8; TRANSFORM_SIZE as usize];
    nan[..4].copy_from_slice(&f32::NAN.to_ne_bytes());

    let cases: Vec<(Vec<Value>, DynError)> = vec![
        (
            vec![Value::tag(keys[TA]), Value::tag(other)],
            DynError::Unknown(other),
        ),
        (
            vec![
                Value::new(keys[SM], &four),
                Value::tag(keys[TA]),
                Value::tag(keys[TA]),
            ],
            DynError::Duplicate(keys[TA]),
        ),
        (
            vec![Value::new(keys[SM], &four[..3])],
            DynError::SizeMismatch {
                component: keys[SM],
                expected: 4,
                actual: 3,
            },
        ),
        (
            vec![Value::new(keys[TA], &four)],
            DynError::SizeMismatch {
                component: keys[TA],
                expected: 0,
                actual: 4,
            },
        ),
        (
            vec![Value::new(keys[SM], &four), Value::new(keys[T], &nan)],
            DynError::Refused(keys[T]),
        ),
    ];
    for (values, err) in cases {
        assert_eq!(reg.spawn(&mut w, &values), Err(err));
        let target = reg.spawn(&mut w, &[]).unwrap();
        assert_eq!(reg.insert(&mut w, target, &values), Err(err));
        w.despawn(target).unwrap();
    }
    assert_eq!(w.len(), 0);
    assert_eq!(w.archetypes().len(), archetypes, "no archetype was made");
    let next = reg.spawn(&mut w, &[]).unwrap();
    assert_eq!(next.id(), first.id(), "no id was taken");

    let mut many_reg = Registry::new();
    let many: Vec<Value> = (0..=MAX_BUNDLE)
        .map(|i| Value::tag(many_reg.register(&format!("m{i}"), 0, 1).unwrap()))
        .collect();
    let mut w = World::new();
    assert_eq!(
        many_reg.spawn(&mut w, &many),
        Err(DynError::BundleTooLarge(MAX_BUNDLE + 1))
    );
    assert_eq!((w.len(), w.archetypes().len()), (0, 1));
    assert!(many_reg.spawn(&mut w, &many[..MAX_BUNDLE]).is_ok());
}

#[test]
fn values_need_a_valued_component_of_the_right_size() {
    let (reg, keys) = registry();
    let mut w = World::new();
    let e = reg
        .spawn(
            &mut w,
            &[Value::tag(keys[TA]), Value::new(keys[SM], &[0; 4])],
        )
        .unwrap();
    assert_eq!(
        reg.get(&w, e, keys[TA], &mut []),
        Err(DynError::NoValue(keys[TA]))
    );
    assert_eq!(
        reg.set(&mut w, e, keys[TA], &[]),
        Err(DynError::NoValue(keys[TA]))
    );
    assert_eq!(
        reg.get(&w, e, keys[ST], &mut []),
        Err(DynError::NoValue(keys[ST])),
        "a native tag has no value either"
    );
    assert_eq!(
        reg.get(&w, e, keys[SM], &mut [0; 8]),
        Err(DynError::SizeMismatch {
            component: keys[SM],
            expected: 4,
            actual: 8
        })
    );
    let mut nan = vec![0u8; TRANSFORM_SIZE as usize];
    nan[..4].copy_from_slice(&f32::NAN.to_ne_bytes());
    let t = reg
        .spawn(
            &mut w,
            &[Value::new(keys[T], &[0; TRANSFORM_SIZE as usize])],
        )
        .unwrap();
    assert_eq!(
        reg.set(&mut w, t, keys[T], &nan),
        Err(DynError::Refused(keys[T]))
    );
    assert_eq!(reg.set(&mut w, e, keys[T], &nan), Ok(false), "absent first");
}

#[test]
fn query_terms_are_checked() {
    let (reg, keys) = registry();
    let w = World::new();
    let mut rows = Rows::new();
    assert_eq!(reg.query(&w, &[], &mut rows), Err(DynError::NoRequiredTerm));
    assert_eq!(
        reg.query_count(&w, &[Term::new(keys[TA], Access::Without)]),
        Err(DynError::NoRequiredTerm)
    );
    let many = vec![Term::new(keys[TA], Access::With); MAX_QUERY_TERMS + 1];
    assert_eq!(
        reg.query(&w, &many, &mut rows),
        Err(DynError::TooManyTerms(MAX_QUERY_TERMS + 1))
    );
    assert_eq!(reg.query(&w, &many[..MAX_QUERY_TERMS], &mut rows), Ok(()));
    let mut other = Registry::new();
    for i in 0..10 {
        other.register(&format!("o{i}"), 0, 1).unwrap();
    }
    let unknown = other.key(9).unwrap();
    assert_eq!(
        reg.query(&w, &[Term::new(unknown, Access::Optional)], &mut rows),
        Err(DynError::Unknown(unknown))
    );
}

#[test]
fn rows_are_laid_out_by_alignment() {
    let mut reg = Registry::new();
    let byte = reg.register("byte", 1, 1).unwrap();
    let wide = reg.register("wide", 16, 16).unwrap();
    let word = reg.register("word", 8, 8).unwrap();
    let flag = reg.register("flag", 0, 1).unwrap();
    let mut w = World::new();
    for i in 0..3u8 {
        reg.spawn(
            &mut w,
            &[
                Value::new(byte, &[i]),
                Value::new(wide, &[i; 16]),
                Value::new(word, &u64::from(i).to_ne_bytes()),
            ],
        )
        .unwrap();
    }
    reg.spawn(&mut w, &[Value::new(byte, &[9]), Value::tag(flag)])
        .unwrap();

    let terms = [
        Term::new(byte, Access::Read),
        Term::new(flag, Access::Read),
        Term::new(wide, Access::Optional),
        Term::new(word, Access::Optional),
    ];
    let mut rows = Rows::new();
    reg.query(&w, &terms, &mut rows).unwrap();
    assert_eq!(rows.offsets(), &[0, u32::MAX, 16, 32]);
    assert_eq!(rows.stride(), 48);
    assert_eq!(rows.term_count(), 4);
    assert_eq!(rows.data().as_ptr() as usize % 16, 0);
    // Reading a tag is a filter: only the entity with the flag matches.
    assert_eq!(rows.len(), 1);
    assert_eq!(rows.data().len(), 48);
    assert_eq!(rows.present(), &[1, 1, 0, 0]);
    assert_eq!(rows.value(0, 2), None);
    assert_eq!(&rows.data()[16..48], &[0; 32], "absent values are zeros");

    let terms = [
        Term::new(byte, Access::Read),
        Term::new(wide, Access::Optional),
        Term::new(flag, Access::Without),
    ];
    reg.query(&w, &terms, &mut rows).unwrap();
    assert_eq!(rows.len(), 3);
    assert_eq!(rows.stride(), 32);
    for r in 0..3 {
        assert_eq!(rows.value(r, 0), Some(&[r as u8][..]));
        assert_eq!(rows.value(r, 1), Some(&[r as u8; 16][..]));
        assert!(!rows.has(r, 2));
    }
}

#[test]
fn rows_outlive_changes_to_the_world() {
    let (reg, keys) = registry();
    let mut w = World::new();
    for i in 0..10u32 {
        reg.spawn(&mut w, &[Value::new(keys[SM], &i.to_ne_bytes())])
            .unwrap();
    }
    let mut rows = Rows::new();
    reg.query(&w, &[Term::new(keys[SM], Access::Read)], &mut rows)
        .unwrap();
    // Read-then-act: despawn while walking the copy.
    for r in 0..rows.len() {
        let v = u32::from_ne_bytes(rows.value(r, 0).unwrap().try_into().unwrap());
        if v % 2 == 0 {
            w.despawn(rows.entity(r)).unwrap();
        }
    }
    assert_eq!(w.len(), 5);
    assert_eq!(rows.len(), 10, "the copy is unchanged");
}
