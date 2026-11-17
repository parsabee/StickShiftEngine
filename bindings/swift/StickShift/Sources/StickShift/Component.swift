// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

import CStickShift
import simd

/// A component: plain data an ``Entity`` carries in the engine's ``World``.
///
/// Any bitwise-copyable struct is a component; an empty struct is a tag,
/// which an entity has or lacks but which holds no value.
///
/// ```swift
/// struct Obstacle: Component {}                        // a tag
/// struct TrafficVariant: Component { var index: Int }  // a value
/// ```
///
/// A game's components are registered with the engine on first use, by
/// ``componentName``, and stored in the engine's world as bytes: a value is
/// copied in when it is spawned, inserted or set, and copied out when it is
/// read. Each type is its own component, with its own archetypes, exactly as
/// a Rust type is. The engine's own components (``Transform``,
/// ``RenderComponent``, ``StaticTag`` and ``DynamicTag``) are stored as the
/// engine's Rust types.
///
/// A component holds no references: a class, a `String` or an `Array` is not
/// bitwise-copyable. At most `STICKSHIFT_MAX_COMPONENT_SIZE` (4096) bytes.
public protocol Component: QueryTerm, BitwiseCopyable, Sendable where Queried == Self {
    /// The name the component is registered under: unique per engine. The
    /// default is the module-qualified type name (`String(reflecting:)`),
    /// so two games' `Obstacle`s never meet. Two types with the same name
    /// and size would share one component; a same name with another size
    /// traps.
    static var componentName: String { get }
}

extension Component {
    /// The module-qualified type name.
    public static var componentName: String { String(reflecting: Self.self) }

    @_documentation(visibility: internal)
    public static var _isOptionalTerm: Bool { false }

    @_documentation(visibility: internal)
    public static func _term(_ value: Self?) -> Self {
        guard let value else { fatalError("StickShift: a required query term had no value") }
        return value
    }
}

/// What a ``World/query(_:)`` can ask for: a ``Component``, which every
/// matching entity has, or an `Optional` of one, which is `nil` for the
/// entities that lack it. Only those two conform.
public protocol QueryTerm: Sendable {
    /// The component the term reads.
    associatedtype Queried: Component
    @_documentation(visibility: internal)
    static var _isOptionalTerm: Bool { get }
    @_documentation(visibility: internal)
    static func _term(_ value: Queried?) -> Self
}

extension Optional: QueryTerm where Wrapped: Component {
    /// The wrapped component.
    public typealias Queried = Wrapped

    @_documentation(visibility: internal)
    public static var _isOptionalTerm: Bool { true }

    @_documentation(visibility: internal)
    public static func _term(_ value: Wrapped?) -> Wrapped? { value }
}

// MARK: - The engine's components

/// Where an entity is: the engine's `TransformComponent`, a translation,
/// rotation and scale. Its matrix scales first, then rotates, then
/// translates. The scene's floating-origin rebase shifts every transform.
public struct Transform: Component, Equatable {
    /// The translation.
    public var position: SIMD3<Float>
    /// The rotation, a unit quaternion.
    public var rotation: simd_quatf
    /// The scale along each axis.
    public var scale: SIMD3<Float>

    /// Creates a transform; by default the identity.
    public init(
        position: SIMD3<Float> = .zero,
        rotation: simd_quatf = simd_quatf(ix: 0, iy: 0, iz: 0, r: 1),
        scale: SIMD3<Float> = .one
    ) {
        self.position = position
        self.rotation = rotation
        self.scale = scale
    }

    /// The transform's matrix, exactly as the engine computes it
    /// (`stickshift_transform_to_mat4`).
    public var matrix: simd_float4x4 {
        simd_float4x4(stickshift_transform_to_mat4(c))
    }

    init(_ t: StickShiftTransform) {
        position = SIMD3(t.position)
        rotation = simd_quatf(ix: t.rotation.x, iy: t.rotation.y, iz: t.rotation.z, r: t.rotation.w)
        scale = SIMD3(t.scale)
    }

    var c: StickShiftTransform {
        StickShiftTransform(position: position, rotation: rotation, scale: scale)
    }
}

/// How the engine's own renderer would draw an entity: a colour and a
/// built-in shape. The engine's `RenderComponent`.
///
/// A game that draws its world itself can still give its entities one: like
/// any component, it decides which archetype an entity lands in, and so the
/// order queries walk entities in.
public struct RenderComponent: Component, Equatable {
    /// A built-in shape.
    public enum Shape: UInt32, Sendable, Equatable, BitwiseCopyable {
        /// A unit cube.
        case cube = 0
        /// A unit sphere.
        case sphere = 1
        /// A mesh made by Rust code. Reads only: a Swift game cannot make one.
        case mesh = 2
    }

    /// Linear RGB, `0...1`.
    public var color: SIMD3<Float>
    /// The shape.
    public var shape: Shape

    /// Creates a render component.
    public init(color: SIMD3<Float>, shape: Shape) {
        self.color = color
        self.shape = shape
    }

    /// A cube of `color`: the engine's `RenderComponent::cube`.
    public static func cube(_ color: SIMD3<Float>) -> RenderComponent {
        RenderComponent(color: color, shape: .cube)
    }

    /// A sphere of `color`: the engine's `RenderComponent::sphere`.
    public static func sphere(_ color: SIMD3<Float>) -> RenderComponent {
        RenderComponent(color: color, shape: .sphere)
    }

    init(_ r: StickShiftRenderComponent) {
        color = SIMD3(r.color)
        shape = Shape(rawValue: r.shape) ?? .mesh
    }

    var c: StickShiftRenderComponent {
        StickShiftRenderComponent(color: color.tuple, shape: shape.rawValue)
    }
}

/// The engine's `StaticTag`: an entity that never moves by itself.
public struct StaticTag: Component, Equatable {
    /// Creates the tag.
    public init() {}
}

/// The engine's `DynamicTag`: an entity the game moves.
public struct DynamicTag: Component, Equatable {
    /// Creates the tag.
    public init() {}
}

/// An entity of an engine's ``World``: hecs `Entity::to_bits`, its generation
/// above its id. Ids are reused, last freed first, with a bumped generation,
/// so an entity that was despawned reads as gone, never as another entity.
/// Valid only in the engine that issued it.
public struct Entity: Hashable, Sendable, CustomStringConvertible {
    /// The engine's bits: `generation << 32 | id`. Never 0.
    public let bits: UInt64

    /// The entity with these bits (from ``bits``, in a trace or a fixture).
    public init(bits: UInt64) {
        self.bits = bits
    }

    public var description: String {
        "Entity(\(bits & 0xffff_ffff)v\(bits >> 32))"
    }

    var c: StickShiftEntity { StickShiftEntity(bits: bits) }
}

// MARK: - Registration and the bytes a component crosses as

/// How one component type crosses the C API.
struct ComponentInfo {
    enum Kind {
        /// ``Transform``, as a `StickShiftTransform`.
        case transform
        /// ``RenderComponent``, as a `StickShiftRenderComponent`.
        case render
        /// No value.
        case tag
        /// A game component's bytes.
        case value(size: Int)
    }

    let id: StickShiftComponent
    let kind: Kind

    /// The size C knows the component by.
    var size: Int {
        switch kind {
        case .transform: MemoryLayout<StickShiftTransform>.size
        case .render: MemoryLayout<StickShiftRenderComponent>.size
        case .tag: 0
        case let .value(size): size
        }
    }

    var isTag: Bool {
        if case .tag = kind { return true }
        return false
    }

    /// Read a `C` from the bytes C wrote for it (nothing, for a tag).
    func decode<C: Component>(_: C.Type, from bytes: UnsafeRawPointer?) -> C {
        switch kind {
        case .transform:
            let t = Transform(bytes!.loadUnaligned(as: StickShiftTransform.self))
            return unsafeBitCast(t, to: C.self)
        case .render:
            let r = RenderComponent(bytes!.loadUnaligned(as: StickShiftRenderComponent.self))
            return unsafeBitCast(r, to: C.self)
        case .tag:
            // No bytes: an empty struct reads none.
            var none: UInt8 = 0
            return withUnsafeBytes(of: &none) { $0.baseAddress!.loadUnaligned(as: C.self) }
        case .value:
            return bytes!.loadUnaligned(as: C.self)
        }
    }

    /// Append the bytes C reads for `value` to `buffer`, 16-byte aligned, and
    /// return where they start.
    func encode<C: Component>(_ value: C, into buffer: inout [UInt8]) -> Int {
        let start = (buffer.count + 15) & ~15
        buffer.append(contentsOf: repeatElement(0, count: start - buffer.count))
        switch kind {
        case .transform:
            var t = unsafeBitCast(value, to: Transform.self).c
            withUnsafeBytes(of: &t) { buffer.append(contentsOf: $0) }
        case .render:
            var r = unsafeBitCast(value, to: RenderComponent.self).c
            withUnsafeBytes(of: &r) { buffer.append(contentsOf: $0) }
        case .tag:
            break
        case .value:
            withUnsafeBytes(of: value) { buffer.append(contentsOf: $0) }
        }
        return start
    }
}

/// An engine's components by Swift type, registered on first use.
@MainActor
final class ComponentRegistry {
    private var known: [ObjectIdentifier: ComponentInfo] = [
        ObjectIdentifier(Transform.self): ComponentInfo(
            id: StickShiftComponent(id: UInt32(STICKSHIFT_COMPONENT_TRANSFORM)), kind: .transform),
        ObjectIdentifier(RenderComponent.self): ComponentInfo(
            id: StickShiftComponent(id: UInt32(STICKSHIFT_COMPONENT_RENDER)), kind: .render),
        ObjectIdentifier(StaticTag.self): ComponentInfo(
            id: StickShiftComponent(id: UInt32(STICKSHIFT_COMPONENT_STATIC_TAG)), kind: .tag),
        ObjectIdentifier(DynamicTag.self): ComponentInfo(
            id: StickShiftComponent(id: UInt32(STICKSHIFT_COMPONENT_DYNAMIC_TAG)), kind: .tag),
    ]

    /// How many game components this engine registered from Swift.
    private(set) var registrations = 0

    /// `C`'s component on `raw`, registering it on its first use. Registering
    /// is allowed in every scope and touches no world state.
    func info<C: Component>(_: C.Type, _ raw: OpaquePointer) -> ComponentInfo {
        let key = ObjectIdentifier(C.self)
        if let info = known[key] { return info }
        let size = MemoryLayout<C>.size
        let name = C.componentName
        var out = StickShiftComponent()
        let result = name.withCString { name in
            var desc = StickShiftComponentDesc(
                name: name,
                size: UInt32(clamping: size),
                align: UInt32(MemoryLayout<C>.alignment)
            )
            return stickshift_component_register(raw, &desc, &out)
        }
        if result < 0 {
            misuse("registering the component \(name) (\(size) bytes)", result)
        }
        let info = ComponentInfo(id: out, kind: size == 0 ? .tag : .value(size: size))
        known[key] = info
        registrations += 1
        return info
    }
}
