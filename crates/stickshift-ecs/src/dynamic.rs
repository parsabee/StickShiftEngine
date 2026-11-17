// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

//! Components registered at run time, for hosts that are not written in Rust.
//!
//! hecs keys archetypes by [`TypeId`], and a host behind the C API cannot define
//! Rust types. So each component such a host registers is given a type of its own
//! from one of two macro-generated families of slot types:
//!
//! - [`Tag<N>`]: zero-sized, for a component with no value.
//! - [`Blob<N>`]: the value's bytes, kept inline up to [`INLINE_PAYLOAD`] bytes
//!   and boxed above that.
//!
//! Each family has [`SLOTS`] types. Every registered component therefore has its
//! own `TypeId`, and the world partitions and orders archetypes exactly as it does
//! for Rust types:
//!
//! - [`Registry::spawn`] builds the whole bundle with an [`EntityBuilder`] and
//!   spawns it once. The built bundle reaches the archetype that a Rust tuple with
//!   the same set of types reaches, and a new archetype is created in first-use
//!   order. The entity is the one `World::spawn` allocates, so ids are reused
//!   last-freed first, with a bumped generation.
//! - [`Registry::insert`] is one `World::insert` of a built bundle, and
//!   [`Registry::remove`] is `World::remove_one` of the component's own type. Both
//!   move the entity exactly as the typed calls do.
//! - [`Registry::query`] walks `World::archetypes` in index order, then each
//!   archetype's rows in order. That is the order a typed hecs query yields.
//!
//! Engine components keep their Rust types. [`Registry::register_native`] gives
//! one a byte form through a [`Codec`], so Rust code that uses the type directly
//! (a scene's rebase over `TransformComponent`, say) sees what this module wrote.
//!
//! Registering touches no world state, so the time a component is registered never
//! changes archetype order. Keys are local to the registry that issued them, and a
//! world takes its registered components from one registry.
//!
//! # Example
//!
//! ```rust
//! use stickshift_ecs::dynamic::{Access, Registry, Rows, Term, Value};
//! use stickshift_ecs::hecs::World;
//!
//! let mut registry = Registry::new();
//! let health = registry.register("Health", 4, 4).unwrap();
//! let marked = registry.register("Marked", 0, 1).unwrap();
//!
//! let mut world = World::new();
//! let e = registry
//!     .spawn(&mut world, &[Value::new(health, &7u32.to_ne_bytes()), Value::tag(marked)])
//!     .unwrap();
//!
//! let mut value = [0u8; 4];
//! assert!(registry.get(&world, e, health, &mut value).unwrap());
//! assert_eq!(u32::from_ne_bytes(value), 7);
//!
//! let mut rows = Rows::new();
//! let terms = [Term::new(health, Access::Read), Term::new(marked, Access::With)];
//! registry.query(&world, &terms, &mut rows).unwrap();
//! assert_eq!(rows.len(), 1);
//! assert_eq!(rows.entity(0), e);
//! assert_eq!(rows.value(0, 0), Some(&7u32.to_ne_bytes()[..]));
//! ```

use std::any::TypeId;
use std::collections::HashMap;
use std::fmt;

use hecs::{Archetype, Component, Entity, EntityBuilder, World};

/// Slot types in each family ([`Tag<N>`] and [`Blob<N>`]).
///
/// Each slot type costs about 5.6 KB of optimized code, almost all of it hecs
/// generics (`World::remove_one` most of all), so the families are kept at 128
/// rather than 256. The crate README has the measurements.
pub const SLOTS: usize = 128;

/// The most game components one registry holds, tags and data together.
pub const MAX_GAME_COMPONENTS: usize = SLOTS;

/// The most bytes in one component value.
pub const MAX_COMPONENT_SIZE: u32 = 4096;

/// The largest alignment a component may declare. Query rows honour it.
pub const MAX_ALIGN: u32 = 16;

/// Values up to this many bytes are stored inside the [`Blob<N>`]; larger ones are
/// boxed.
pub const INLINE_PAYLOAD: usize = 32;

/// The most components in one spawn or insert.
pub const MAX_BUNDLE: usize = 32;

/// The most terms in one query.
pub const MAX_QUERY_TERMS: usize = 16;

/// The slot type of a registered component with no value.
///
/// The `SLOT`th tag a [`Registry`] registers is stored as `Tag<SLOT>`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Tag<const SLOT: u16>;

/// The slot type of a registered component with a value: its bytes.
///
/// The `SLOT`th data component a [`Registry`] registers is stored as `Blob<SLOT>`.
/// Only this module builds one, always with the registered size.
pub struct Blob<const SLOT: u16>(Payload);

impl<const SLOT: u16> Blob<SLOT> {
    /// The value's bytes: as many as the component's registered size.
    pub fn bytes(&self) -> &[u8] {
        self.0.bytes()
    }
}

impl<const SLOT: u16> fmt::Debug for Blob<SLOT> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Blob").field(&self.bytes()).finish()
    }
}

enum Payload {
    Inline {
        len: u8,
        bytes: [u8; INLINE_PAYLOAD],
    },
    Boxed(Box<[u8]>),
}

impl Payload {
    fn new(src: &[u8]) -> Self {
        if src.len() <= INLINE_PAYLOAD {
            let mut bytes = [0; INLINE_PAYLOAD];
            bytes[..src.len()].copy_from_slice(src);
            Payload::Inline {
                len: src.len() as u8,
                bytes,
            }
        } else {
            Payload::Boxed(src.into())
        }
    }

    fn bytes(&self) -> &[u8] {
        match self {
            Payload::Inline { len, bytes } => &bytes[..usize::from(*len)],
            Payload::Boxed(bytes) => bytes,
        }
    }

    fn bytes_mut(&mut self) -> &mut [u8] {
        match self {
            Payload::Inline { len, bytes } => &mut bytes[..usize::from(*len)],
            Payload::Boxed(bytes) => bytes,
        }
    }
}

/// The byte form of an engine component that keeps its Rust type.
///
/// Implement it on a type of your own (a codec) for a component type, then
/// register it with [`Registry::register_native`]. The byte slices passed in are
/// always exactly the registered size.
pub trait Codec: 'static {
    /// The component's Rust type.
    type Component: Component;

    /// Writes `value`'s byte form into `out`.
    fn encode(value: &Self::Component, out: &mut [u8]);

    /// Builds a value from its byte form, or `None` if the bytes describe no value
    /// this codec accepts.
    fn decode(bytes: &[u8]) -> Option<Self::Component>;

    /// Overwrites `value` from its byte form. Returns `false`, leaving `value` as it
    /// was, if the bytes describe no value this codec accepts.
    fn assign(value: &mut Self::Component, bytes: &[u8]) -> bool {
        match Self::decode(bytes) {
            Some(new) => {
                *value = new;
                true
            }
            None => false,
        }
    }
}

/// Where one term's values go in each query row.
#[derive(Clone, Copy)]
struct Column {
    stride: usize,
    offset: usize,
    size: usize,
}

/// The outcome of overwriting a value.
enum Write {
    Done,
    Absent,
    Refused,
}

/// The typed operations of one component type. Every function takes bytes of the
/// registered size; the value functions are never called for a size of 0.
#[derive(Clone, Copy)]
struct VTable {
    type_id: fn() -> TypeId,
    /// Adds a value built from the bytes; `false` if a codec refused them.
    add: fn(&mut EntityBuilder, &[u8]) -> bool,
    /// `World::remove_one` of the type; `false` if the entity lacks it or is gone.
    remove: fn(&mut World, Entity) -> bool,
    /// Copies the value out; `false` if the entity lacks it or is gone.
    read: fn(&World, Entity, &mut [u8]) -> bool,
    write: fn(&mut World, Entity, &[u8]) -> Write,
    /// Copies the archetype's column into its rows; nothing if it has no such column.
    column: fn(&Archetype, &mut [u8], Column),
}

fn type_of<T: 'static>() -> TypeId {
    TypeId::of::<T>()
}

fn remove_typed<T: Component>(world: &mut World, entity: Entity) -> bool {
    world.remove_one::<T>(entity).is_ok()
}

fn no_read(_: &World, _: Entity, _: &mut [u8]) -> bool {
    false
}

fn no_write(_: &mut World, _: Entity, _: &[u8]) -> Write {
    Write::Absent
}

fn no_column(_: &Archetype, _: &mut [u8], _: Column) {}

fn tag_add<const N: u16>(builder: &mut EntityBuilder, _: &[u8]) -> bool {
    builder.add(Tag::<N>);
    true
}

const fn tag_vtable<const N: u16>() -> VTable {
    VTable {
        type_id: type_of::<Tag<N>>,
        add: tag_add::<N>,
        remove: remove_typed::<Tag<N>>,
        read: no_read,
        write: no_write,
        column: no_column,
    }
}

fn blob_add<const N: u16>(builder: &mut EntityBuilder, data: &[u8]) -> bool {
    builder.add(Blob::<N>(Payload::new(data)));
    true
}

fn blob_read<const N: u16>(world: &World, entity: Entity, out: &mut [u8]) -> bool {
    match world.get::<&Blob<N>>(entity) {
        Ok(value) => {
            out.copy_from_slice(value.bytes());
            true
        }
        Err(_) => false,
    }
}

fn blob_write<const N: u16>(world: &mut World, entity: Entity, data: &[u8]) -> Write {
    match world.get::<&mut Blob<N>>(entity) {
        Ok(mut value) => {
            value.0.bytes_mut().copy_from_slice(data);
            Write::Done
        }
        Err(_) => Write::Absent,
    }
}

fn blob_column<const N: u16>(archetype: &Archetype, out: &mut [u8], at: Column) {
    if let Some(column) = archetype.get::<&Blob<N>>() {
        for (row, value) in column.iter().enumerate() {
            let start = row * at.stride + at.offset;
            out[start..start + at.size].copy_from_slice(value.bytes());
        }
    }
}

const fn blob_vtable<const N: u16>() -> VTable {
    VTable {
        type_id: type_of::<Blob<N>>,
        add: blob_add::<N>,
        remove: remove_typed::<Blob<N>>,
        read: blob_read::<N>,
        write: blob_write::<N>,
        column: blob_column::<N>,
    }
}

fn native_add<C: Codec>(builder: &mut EntityBuilder, data: &[u8]) -> bool {
    match C::decode(data) {
        Some(value) => {
            builder.add(value);
            true
        }
        None => false,
    }
}

fn native_read<C: Codec>(world: &World, entity: Entity, out: &mut [u8]) -> bool {
    match world.get::<&C::Component>(entity) {
        Ok(value) => {
            C::encode(&value, out);
            true
        }
        Err(_) => false,
    }
}

fn native_write<C: Codec>(world: &mut World, entity: Entity, data: &[u8]) -> Write {
    match world.get::<&mut C::Component>(entity) {
        Ok(mut value) => {
            if C::assign(&mut value, data) {
                Write::Done
            } else {
                Write::Refused
            }
        }
        Err(_) => Write::Absent,
    }
}

fn native_column<C: Codec>(archetype: &Archetype, out: &mut [u8], at: Column) {
    if let Some(column) = archetype.get::<&C::Component>() {
        for (row, value) in column.iter().enumerate() {
            let start = row * at.stride + at.offset;
            C::encode(value, &mut out[start..start + at.size]);
        }
    }
}

fn native_vtable<C: Codec>() -> VTable {
    VTable {
        type_id: type_of::<C::Component>,
        add: native_add::<C>,
        remove: remove_typed::<C::Component>,
        read: native_read::<C>,
        write: native_write::<C>,
        column: native_column::<C>,
    }
}

/// Builds a [`SlotTable`] of `$make::<N>()` for N in `0..SLOTS`. The row list
/// must have `SLOTS / 16` entries; the table's type rejects any other count.
macro_rules! slot_table {
    ($make:ident) => {
        slot_table!(@rows $make; 0 1 2 3 4 5 6 7)
    };
    (@rows $make:ident; $($hi:literal)*) => {
        [$(slot_table!(@cells $make; $hi; 0 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15)),*]
    };
    (@cells $make:ident; $hi:literal; $($lo:literal)*) => {
        [$($make::<{ $hi * 16 + $lo }>()),*]
    };
}

type SlotTable = [[VTable; 16]; SLOTS / 16];

static TAG_SLOTS: SlotTable = slot_table!(tag_vtable);
static BLOB_SLOTS: SlotTable = slot_table!(blob_vtable);

fn slot(table: &SlotTable, slot: usize) -> VTable {
    table[slot / 16][slot % 16]
}

/// A registered component, local to the [`Registry`] that issued it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Key(u32);

impl Key {
    /// The registration index: 0 for the first component registered, and so on.
    pub fn index(self) -> u32 {
        self.0
    }
}

/// How a registered component is stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A [`Tag<N>`]: no value.
    Tag,
    /// A [`Blob<N>`]: the value's bytes.
    Data,
    /// An engine component's own Rust type, through a [`Codec`].
    Native,
}

/// What a [`Registry`] knows about one component.
#[derive(Clone, Copy, Debug)]
pub struct Info<'a> {
    /// The name it was registered under.
    pub name: &'a str,
    /// The bytes of one value; 0 for a component with no value.
    pub size: u32,
    /// The value's alignment in query rows.
    pub align: u32,
    /// How it is stored.
    pub kind: Kind,
    /// The Rust type the world stores it as.
    pub type_id: TypeId,
}

/// Why a registration was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegisterError {
    /// The name is empty.
    EmptyName,
    /// The size is above [`MAX_COMPONENT_SIZE`].
    TooLarge {
        /// The size asked for.
        size: u32,
    },
    /// The alignment is not a power of two up to [`MAX_ALIGN`].
    BadAlign {
        /// The alignment asked for.
        align: u32,
    },
    /// The name, or the native type, is already registered with a different size,
    /// alignment or storage.
    Conflict {
        /// The component registered first.
        existing: Key,
    },
    /// [`MAX_GAME_COMPONENTS`] game components are already registered.
    LimitExceeded,
}

impl fmt::Display for RegisterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyName => write!(f, "a component name must not be empty"),
            Self::TooLarge { size } => write!(
                f,
                "a component of {size} bytes is above the limit of {MAX_COMPONENT_SIZE}"
            ),
            Self::BadAlign { align } => write!(
                f,
                "alignment {align} is not a power of two up to {MAX_ALIGN}"
            ),
            Self::Conflict { existing } => write!(
                f,
                "already registered differently, as component {}",
                existing.0
            ),
            Self::LimitExceeded => write!(
                f,
                "{MAX_GAME_COMPONENTS} game components are already registered"
            ),
        }
    }
}

impl std::error::Error for RegisterError {}

/// Why a world operation was refused. A refused operation changes nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DynError {
    /// The key was not issued by this registry.
    Unknown(Key),
    /// The bytes given are not the component's registered size.
    SizeMismatch {
        /// The component.
        component: Key,
        /// Its registered size.
        expected: u32,
        /// The bytes given.
        actual: usize,
    },
    /// The component appears twice in one bundle.
    Duplicate(Key),
    /// The bundle has more than [`MAX_BUNDLE`] components.
    BundleTooLarge(usize),
    /// The component has no value to read or write.
    NoValue(Key),
    /// A native component's codec refused the bytes.
    Refused(Key),
    /// The query has more than [`MAX_QUERY_TERMS`] terms.
    TooManyTerms(usize),
    /// The query has no term that requires or reads a component.
    NoRequiredTerm,
}

impl fmt::Display for DynError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unknown(key) => write!(f, "component {} is not registered", key.0),
            Self::SizeMismatch {
                component,
                expected,
                actual,
            } => write!(
                f,
                "component {} has {expected} bytes, but {actual} were given",
                component.0
            ),
            Self::Duplicate(key) => write!(f, "component {} appears twice", key.0),
            Self::BundleTooLarge(n) => {
                write!(f, "{n} components is more than the {MAX_BUNDLE} allowed")
            }
            Self::NoValue(key) => write!(f, "component {} has no value", key.0),
            Self::Refused(key) => write!(f, "component {} refused the value", key.0),
            Self::TooManyTerms(n) => {
                write!(f, "{n} terms is more than the {MAX_QUERY_TERMS} allowed")
            }
            Self::NoRequiredTerm => write!(f, "a query needs a term that is not excluded"),
        }
    }
}

impl std::error::Error for DynError {}

/// One component of a bundle: its key and its value's bytes.
#[derive(Clone, Copy, Debug)]
pub struct Value<'a> {
    /// The component.
    pub component: Key,
    /// The value's bytes, exactly the registered size; empty for a component with
    /// no value.
    pub data: &'a [u8],
}

impl<'a> Value<'a> {
    /// A component and its value's bytes.
    pub fn new(component: Key, data: &'a [u8]) -> Self {
        Self { component, data }
    }

    /// A component with no value.
    pub fn tag(component: Key) -> Self {
        Self {
            component,
            data: &[],
        }
    }
}

/// How a query term uses its component.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    /// Required, and its value is copied into the row (hecs `&T`). For a component
    /// with no value it is the same as [`Access::With`].
    Read,
    /// Required, with no value in the row (hecs `With`).
    With,
    /// Excluded (hecs `Without`).
    Without,
    /// Not a filter: the value when present, and a presence flag (hecs `Option<&T>`).
    Optional,
}

/// One term of a query.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Term {
    /// The component.
    pub component: Key,
    /// How the query uses it.
    pub access: Access,
}

impl Term {
    /// A term.
    pub fn new(component: Key, access: Access) -> Self {
        Self { component, access }
    }
}

/// 16 bytes, so a row buffer starts at an address any component alignment accepts.
#[derive(Clone, Copy)]
#[repr(C, align(16))]
struct Chunk([u8; 16]);

const _: () = assert!(std::mem::size_of::<Chunk>() == 16 && MAX_ALIGN as usize == 16);

/// The rows of one query, copied out of the world.
///
/// Nothing in it borrows the world, so the world may change while the rows are
/// read. Reuse one buffer across queries to keep its allocations.
///
/// Each row is `stride` bytes. A term's value sits at its offset in the row, which
/// is a multiple of the component's alignment, and the buffer starts 16-byte
/// aligned. An optional term that is absent leaves zeros there.
#[derive(Default)]
pub struct Rows {
    entities: Vec<u64>,
    data: Vec<Chunk>,
    data_len: usize,
    stride: usize,
    offsets: Vec<u32>,
    sizes: Vec<u32>,
    present: Vec<u8>,
}

impl Rows {
    /// An empty buffer.
    pub fn new() -> Self {
        Self::default()
    }

    /// The matching entities.
    pub fn len(&self) -> usize {
        self.entities.len()
    }

    /// Whether no entity matched.
    pub fn is_empty(&self) -> bool {
        self.entities.is_empty()
    }

    /// The terms of the query that filled it.
    pub fn term_count(&self) -> usize {
        self.offsets.len()
    }

    /// The matching entities as `Entity::to_bits`, in query order.
    pub fn entity_bits(&self) -> &[u64] {
        &self.entities
    }

    /// The entity of row `row`.
    ///
    /// # Panics
    ///
    /// If `row` is not below [`len`](Self::len).
    pub fn entity(&self, row: usize) -> Entity {
        Entity::from_bits(self.entities[row]).expect("a query row holds a live entity")
    }

    /// The bytes of one row.
    pub fn stride(&self) -> usize {
        self.stride
    }

    /// All rows, `len() * stride()` bytes, starting 16-byte aligned.
    pub fn data(&self) -> &[u8] {
        // SAFETY: `Chunk` is 16 initialized bytes with no padding, so the chunks are
        // `16 * data.len()` initialized bytes, and `data_len` never exceeds that.
        unsafe { std::slice::from_raw_parts(self.data.as_ptr().cast::<u8>(), self.data_len) }
    }

    /// Per term, the byte offset of its value in a row, or `u32::MAX` if the row
    /// holds no value for it.
    pub fn offsets(&self) -> &[u32] {
        &self.offsets
    }

    /// `len() * term_count()` flags: 1 where the row's entity has the term's
    /// component.
    pub fn present(&self) -> &[u8] {
        &self.present
    }

    /// Whether row `row`'s entity has term `term`'s component.
    ///
    /// # Panics
    ///
    /// If `row` or `term` is out of range.
    pub fn has(&self, row: usize, term: usize) -> bool {
        assert!(term < self.term_count(), "term {term} out of range");
        self.present[row * self.term_count() + term] != 0
    }

    /// The value of term `term` in row `row`: `None` if the term holds no value, or
    /// if it is optional and the entity lacks it.
    ///
    /// # Panics
    ///
    /// If `row` or `term` is out of range.
    pub fn value(&self, row: usize, term: usize) -> Option<&[u8]> {
        if !self.has(row, term) || self.offsets[term] == u32::MAX {
            return None;
        }
        let start = row * self.stride + self.offsets[term] as usize;
        Some(&self.data()[start..start + self.sizes[term] as usize])
    }

    /// Empties the buffer, keeping its allocations.
    pub fn clear(&mut self) {
        self.entities.clear();
        self.data.clear();
        self.data_len = 0;
        self.stride = 0;
        self.offsets.clear();
        self.sizes.clear();
        self.present.clear();
    }

    /// Appends `n` zeroed rows and returns their bytes.
    fn grow(&mut self, n: usize) -> &mut [u8] {
        let start = self.data_len;
        self.data_len += n * self.stride;
        self.data.resize(self.data_len.div_ceil(16), Chunk([0; 16]));
        // SAFETY: as in `data`; the slice is uniquely borrowed through `&mut self`.
        let all = unsafe {
            std::slice::from_raw_parts_mut(self.data.as_mut_ptr().cast::<u8>(), self.data_len)
        };
        &mut all[start..]
    }
}

struct Entry {
    name: Box<str>,
    size: u32,
    align: u32,
    kind: Kind,
    type_id: TypeId,
    vt: VTable,
}

/// One resolved query term.
struct Planned<'r> {
    entry: &'r Entry,
    access: Access,
    /// The value's offset in a row, if the row holds one.
    offset: Option<usize>,
}

/// The components registered for one world, and the operations that use them.
///
/// Operations take the world explicitly. Every operation checks all its input
/// before it touches the world, so a refused one changes nothing.
#[derive(Default)]
pub struct Registry {
    entries: Vec<Entry>,
    by_name: HashMap<Box<str>, Key>,
    by_type: HashMap<TypeId, Key>,
    tags: usize,
    blobs: usize,
}

impl Registry {
    /// A registry with nothing registered.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a game component: a [`Tag<N>`] for size 0, otherwise a
    /// [`Blob<N>`] of `size` bytes. `align` lays out query rows.
    ///
    /// The same name with the same size and alignment returns the same key. A
    /// different size or alignment is [`RegisterError::Conflict`], and so is a name
    /// already taken by a native component.
    pub fn register(&mut self, name: &str, size: u32, align: u32) -> Result<Key, RegisterError> {
        check_layout(name, size, align)?;
        let kind = if size == 0 { Kind::Tag } else { Kind::Data };
        if let Some(&key) = self.by_name.get(name) {
            let e = &self.entries[key.0 as usize];
            return if e.size == size && e.align == align && e.kind == kind {
                Ok(key)
            } else {
                Err(RegisterError::Conflict { existing: key })
            };
        }
        if self.tags + self.blobs >= MAX_GAME_COMPONENTS {
            return Err(RegisterError::LimitExceeded);
        }
        let vt = if size == 0 {
            self.tags += 1;
            slot(&TAG_SLOTS, self.tags - 1)
        } else {
            self.blobs += 1;
            slot(&BLOB_SLOTS, self.blobs - 1)
        };
        Ok(self.push(name, size, align, kind, vt))
    }

    /// Registers an engine component that keeps its Rust type, `C::Component`, with
    /// `size` bytes in its byte form (0 for no value). It takes no slot and does not
    /// count toward [`MAX_GAME_COMPONENTS`].
    ///
    /// Registering the same name and type again with the same size and alignment
    /// returns the same key. A name or a type already registered differently is
    /// [`RegisterError::Conflict`].
    pub fn register_native<C: Codec>(
        &mut self,
        name: &str,
        size: u32,
        align: u32,
    ) -> Result<Key, RegisterError> {
        check_layout(name, size, align)?;
        let type_id = TypeId::of::<C::Component>();
        if let Some(&key) = self.by_name.get(name) {
            let e = &self.entries[key.0 as usize];
            return if e.size == size
                && e.align == align
                && e.kind == Kind::Native
                && e.type_id == type_id
            {
                Ok(key)
            } else {
                Err(RegisterError::Conflict { existing: key })
            };
        }
        if let Some(&existing) = self.by_type.get(&type_id) {
            return Err(RegisterError::Conflict { existing });
        }
        Ok(self.push(name, size, align, Kind::Native, native_vtable::<C>()))
    }

    fn push(&mut self, name: &str, size: u32, align: u32, kind: Kind, vt: VTable) -> Key {
        let key = Key(self.entries.len() as u32);
        let type_id = (vt.type_id)();
        self.entries.push(Entry {
            name: name.into(),
            size,
            align,
            kind,
            type_id,
            vt,
        });
        self.by_name.insert(name.into(), key);
        self.by_type.insert(type_id, key);
        key
    }

    /// The components registered, natives included.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether nothing is registered.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The game components registered (tags and data, not natives).
    pub fn game_count(&self) -> usize {
        self.tags + self.blobs
    }

    /// The key with registration index `index`, if there is one.
    pub fn key(&self, index: u32) -> Option<Key> {
        ((index as usize) < self.entries.len()).then_some(Key(index))
    }

    /// The key registered under `name`.
    pub fn lookup(&self, name: &str) -> Option<Key> {
        self.by_name.get(name).copied()
    }

    /// The key whose values the world stores as type `id`.
    pub fn key_of_type(&self, id: TypeId) -> Option<Key> {
        self.by_type.get(&id).copied()
    }

    /// What is registered under `key`.
    pub fn info(&self, key: Key) -> Option<Info<'_>> {
        self.entries.get(key.0 as usize).map(|e| Info {
            name: &e.name,
            size: e.size,
            align: e.align,
            kind: e.kind,
            type_id: e.type_id,
        })
    }

    fn entry(&self, key: Key) -> Result<&Entry, DynError> {
        self.entries
            .get(key.0 as usize)
            .ok_or(DynError::Unknown(key))
    }

    /// The entry of a component whose value is read or written.
    fn valued(&self, key: Key, len: usize) -> Result<&Entry, DynError> {
        let e = self.entry(key)?;
        if e.size == 0 {
            return Err(DynError::NoValue(key));
        }
        if len != e.size as usize {
            return Err(DynError::SizeMismatch {
                component: key,
                expected: e.size,
                actual: len,
            });
        }
        Ok(e)
    }

    /// Checks a bundle, then builds it. The world is not touched.
    fn build(&self, values: &[Value<'_>]) -> Result<EntityBuilder, DynError> {
        if values.len() > MAX_BUNDLE {
            return Err(DynError::BundleTooLarge(values.len()));
        }
        for (i, v) in values.iter().enumerate() {
            let e = self.entry(v.component)?;
            if v.data.len() != e.size as usize {
                return Err(DynError::SizeMismatch {
                    component: v.component,
                    expected: e.size,
                    actual: v.data.len(),
                });
            }
            if values[..i].iter().any(|w| w.component == v.component) {
                return Err(DynError::Duplicate(v.component));
            }
        }
        let mut builder = EntityBuilder::new();
        for v in values {
            let e = &self.entries[v.component.0 as usize];
            if !(e.vt.add)(&mut builder, v.data) {
                return Err(DynError::Refused(v.component));
            }
        }
        Ok(builder)
    }

    /// Spawns one entity with the whole bundle, in one `World::spawn`.
    ///
    /// The order of `values` does not matter: the archetype is the one for the set
    /// of components. An unknown or duplicate component, a size mismatch, a bundle
    /// above [`MAX_BUNDLE`] or a refused native value spawns nothing.
    pub fn spawn(&self, world: &mut World, values: &[Value<'_>]) -> Result<Entity, DynError> {
        let mut builder = self.build(values)?;
        Ok(world.spawn(builder.build()))
    }

    /// Adds the bundle's components to `entity`, replacing values it already has,
    /// in one `World::insert`. That moves the entity to the end of its new archetype
    /// unless the set of components is unchanged.
    ///
    /// `Ok(false)` if the entity is gone; then nothing changes.
    pub fn insert(
        &self,
        world: &mut World,
        entity: Entity,
        values: &[Value<'_>],
    ) -> Result<bool, DynError> {
        let mut builder = self.build(values)?;
        Ok(world.insert(entity, builder.build()).is_ok())
    }

    /// Removes one component from `entity` with `World::remove_one` of its type,
    /// which moves the entity as the typed call does.
    ///
    /// `Ok(false)` if the entity is gone or lacks the component.
    pub fn remove(
        &self,
        world: &mut World,
        entity: Entity,
        component: Key,
    ) -> Result<bool, DynError> {
        let e = self.entry(component)?;
        Ok((e.vt.remove)(world, entity))
    }

    /// Whether `entity` has the component; `false` if it is gone.
    pub fn has(&self, world: &World, entity: Entity, component: Key) -> Result<bool, DynError> {
        let e = self.entry(component)?;
        Ok(world
            .entity(entity)
            .is_ok_and(|r| r.component_types().any(|t| t == e.type_id)))
    }

    /// Copies the component's value into `out`, which must be its registered size.
    ///
    /// `Ok(false)` if the entity is gone or lacks the component; then `out` is
    /// unchanged.
    pub fn get(
        &self,
        world: &World,
        entity: Entity,
        component: Key,
        out: &mut [u8],
    ) -> Result<bool, DynError> {
        let e = self.valued(component, out.len())?;
        Ok((e.vt.read)(world, entity, out))
    }

    /// Overwrites the component's value in place. The entity does not move.
    ///
    /// `Ok(false)` if the entity is gone or lacks the component.
    pub fn set(
        &self,
        world: &mut World,
        entity: Entity,
        component: Key,
        data: &[u8],
    ) -> Result<bool, DynError> {
        let e = self.valued(component, data.len())?;
        match (e.vt.write)(world, entity, data) {
            Write::Done => Ok(true),
            Write::Absent => Ok(false),
            Write::Refused => Err(DynError::Refused(component)),
        }
    }

    fn plan(&self, terms: &[Term]) -> Result<(Vec<Planned<'_>>, usize), DynError> {
        if terms.len() > MAX_QUERY_TERMS {
            return Err(DynError::TooManyTerms(terms.len()));
        }
        let mut planned = Vec::with_capacity(terms.len());
        let (mut end, mut align) = (0usize, 1usize);
        for t in terms {
            let entry = self.entry(t.component)?;
            let offset = match t.access {
                Access::Read | Access::Optional if entry.size > 0 => {
                    let a = entry.align as usize;
                    let offset = end.next_multiple_of(a);
                    end = offset + entry.size as usize;
                    align = align.max(a);
                    Some(offset)
                }
                _ => None,
            };
            planned.push(Planned {
                entry,
                access: t.access,
                offset,
            });
        }
        if planned.iter().all(|p| p.access == Access::Without) {
            return Err(DynError::NoRequiredTerm);
        }
        Ok((planned, end.next_multiple_of(align)))
    }

    /// Copies every matching entity into `rows`, replacing what it held, in the
    /// order a typed hecs query yields them: archetypes in index order, then rows in
    /// order.
    ///
    /// At least one term must not be [`Access::Without`]. On an error `rows` is
    /// left empty.
    pub fn query(&self, world: &World, terms: &[Term], rows: &mut Rows) -> Result<(), DynError> {
        rows.clear();
        let (planned, stride) = self.plan(terms)?;
        rows.stride = stride;
        rows.offsets.extend(
            planned
                .iter()
                .map(|p| p.offset.map_or(u32::MAX, |o| o as u32)),
        );
        rows.sizes.extend(planned.iter().map(|p| p.entry.size));
        for archetype in world.archetypes() {
            if archetype.is_empty() || !matches(archetype, &planned) {
                continue;
            }
            for &id in archetype.ids() {
                // SAFETY: an id stored in an archetype belongs to a live entity
                // (`World::find_entity_from_id` asks for exactly that).
                let entity = unsafe { world.find_entity_from_id(id) };
                rows.entities.push(entity.to_bits().get());
            }
            let n = archetype.len() as usize;
            for p in &planned {
                let has = match p.access {
                    Access::Read | Access::With => true,
                    Access::Without => false,
                    Access::Optional => archetype.has_dynamic(p.entry.type_id),
                };
                rows.present.push(u8::from(has));
            }
            // The flags just pushed are the first row's; repeat them for the rest.
            let first = rows.present.len() - planned.len();
            for _ in 1..n {
                rows.present
                    .extend_from_within(first..first + planned.len());
            }
            let region = rows.grow(n);
            for p in &planned {
                if let Some(offset) = p.offset {
                    let at = Column {
                        stride,
                        offset,
                        size: p.entry.size as usize,
                    };
                    (p.entry.vt.column)(archetype, region, at);
                }
            }
        }
        Ok(())
    }

    /// Counts the entities [`query`](Self::query) would copy, without copying.
    pub fn query_count(&self, world: &World, terms: &[Term]) -> Result<u32, DynError> {
        let (planned, _) = self.plan(terms)?;
        Ok(world
            .archetypes()
            .filter(|a| matches(a, &planned))
            .map(Archetype::len)
            .sum())
    }
}

fn check_layout(name: &str, size: u32, align: u32) -> Result<(), RegisterError> {
    if name.is_empty() {
        return Err(RegisterError::EmptyName);
    }
    if size > MAX_COMPONENT_SIZE {
        return Err(RegisterError::TooLarge { size });
    }
    if !align.is_power_of_two() || align > MAX_ALIGN {
        return Err(RegisterError::BadAlign { align });
    }
    Ok(())
}

/// Whether an archetype holds what the terms require and nothing they exclude.
fn matches(archetype: &Archetype, planned: &[Planned<'_>]) -> bool {
    planned.iter().all(|p| match p.access {
        Access::Read | Access::With => archetype.has_dynamic(p.entry.type_id),
        Access::Without => !archetype.has_dynamic(p.entry.type_id),
        Access::Optional => true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every slot of both families is its own type, so no two registrations share
    /// archetype identity.
    #[test]
    fn every_slot_is_a_distinct_type() {
        let mut ids: Vec<TypeId> = (0..SLOTS)
            .flat_map(|s| {
                [
                    (slot(&TAG_SLOTS, s).type_id)(),
                    (slot(&BLOB_SLOTS, s).type_id)(),
                ]
            })
            .collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), 2 * SLOTS);
        assert_eq!((slot(&TAG_SLOTS, 37).type_id)(), TypeId::of::<Tag<37>>());
        assert_eq!(
            (slot(&BLOB_SLOTS, SLOTS - 1).type_id)(),
            TypeId::of::<Blob<127>>()
        );
    }

    #[test]
    fn payload_is_inline_up_to_the_limit_and_boxed_above() {
        let small = Payload::new(&[7; INLINE_PAYLOAD]);
        assert!(matches!(small, Payload::Inline { len: 32, .. }));
        assert_eq!(small.bytes(), &[7; INLINE_PAYLOAD]);
        let big = Payload::new(&[9; INLINE_PAYLOAD + 1]);
        assert!(matches!(big, Payload::Boxed(_)));
        assert_eq!(big.bytes(), &[9; INLINE_PAYLOAD + 1]);
    }
}
