// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

import CStickShift
import Foundation

/// Where a resource is in its life.
public enum ResourceState: Sendable, Equatable {
    /// Destroyed, or never valid.
    case invalid
    /// Accepted; the work to make it usable has not finished.
    case pending
    /// Usable in draws.
    case ready
    /// The work failed; draws using it are skipped.
    case failed

    init(_ s: StickShiftResourceState) {
        switch Int32(bitPattern: s) {
        case STICKSHIFT_RESOURCE_STATE_PENDING: self = .pending
        case STICKSHIFT_RESOURCE_STATE_READY: self = .ready
        case STICKSHIFT_RESOURCE_STATE_FAILED: self = .failed
        default: self = .invalid
        }
    }
}

/// A mesh's vertex layout, which also picks the pipeline that draws it.
public enum VertexLayout: Sendable {
    /// Nine floats per vertex: position xyz, normal xyz, linear RGB colour.
    case positionNormalColor
    /// Eight floats per vertex: position xyz, normal xyz, texture uv. Drawn with
    /// the material's base-colour texture.
    case positionNormalUV

    /// Floats per vertex.
    public var floatsPerVertex: Int {
        switch self {
        case .positionNormalColor: 9
        case .positionNormalUV: 8
        }
    }

    var c: StickShiftVertexLayout {
        StickShiftVertexLayout(
            self == .positionNormalColor
                ? STICKSHIFT_VERTEX_LAYOUT_POS_NORMAL_COLOR : STICKSHIFT_VERTEX_LAYOUT_POS_NORMAL_UV
        )
    }
}

/// How a texture's colour channels are encoded.
public enum ColorSpace: Sendable {
    /// sRGB-encoded colour (photos, painted albedo): decoded to linear before
    /// lighting.
    case srgb
    /// Data sampled as stored: font atlases, masks.
    case linear

    var c: StickShiftColorSpace {
        StickShiftColorSpace(self == .srgb ? STICKSHIFT_COLOR_SPACE_SRGB : STICKSHIFT_COLOR_SPACE_LINEAR)
    }
}

/// Run a resource's C destroy for `context`: here on the main thread, and
/// otherwise sent to it, holding the context until then. A context with an
/// engine attached refuses every call from another thread, so a resource
/// whose last reference goes on a background thread would otherwise leak.
/// On the main thread, a destroy the context cannot take now (inside a
/// callback, or a stream pass's closure) is retried once the call returns.
func destroyResource(of context: Context, _ destroy: @escaping @Sendable (OpaquePointer) -> StickShiftResult) {
    if Thread.isMainThread {
        let result = destroy(context.raw)
        guard result == STICKSHIFT_ERROR_BUSY || result == STICKSHIFT_ERROR_WRONG_STATE else { return }
    }
    DispatchQueue.main.async { _ = destroy(context.raw) }
}

/// Bytes passed to the engine, which copies them before the call returns.
private func copied(_ bytes: UnsafeRawBufferPointer) -> StickShiftData {
    StickShiftData(ptr: bytes.baseAddress, len: bytes.count, release: nil, user: nil)
}

/// A triangle mesh on the GPU. Destroyed when you drop the last reference.
@MainActor
public final class Mesh {
    /// The context that owns it.
    public let context: Context
    /// The C handle's id.
    public let id: UInt64
    /// Its vertex layout.
    public let layout: VertexLayout

    init(context: Context, id: UInt64, layout: VertexLayout) {
        self.context = context
        self.id = id
        self.layout = layout
    }

    /// Where the mesh is in its life.
    public var state: ResourceState {
        ResourceState(stickshift_resource_state(context.raw, id))
    }

    deinit {
        let id = id
        destroyResource(of: context) { stickshift_mesh_destroy($0, StickShiftMesh(id: id)) }
    }
}

/// An RGBA8 texture on the GPU, mipmapped. Destroyed when you drop the last
/// reference.
@MainActor
public final class Texture {
    /// The context that owns it.
    public let context: Context
    /// The C handle's id.
    public let id: UInt64
    /// Width, in pixels.
    public let width: Int
    /// Height, in pixels.
    public let height: Int
    /// How its colour channels are encoded.
    public let colorSpace: ColorSpace

    init(context: Context, id: UInt64, width: Int, height: Int, colorSpace: ColorSpace) {
        self.context = context
        self.id = id
        self.width = width
        self.height = height
        self.colorSpace = colorSpace
    }

    /// Where the texture is in its life.
    public var state: ResourceState {
        ResourceState(stickshift_resource_state(context.raw, id))
    }

    var handle: StickShiftTexture { StickShiftTexture(id: id) }

    deinit {
        let id = id
        destroyResource(of: context) { stickshift_texture_destroy($0, StickShiftTexture(id: id)) }
    }
}

/// One glyph of a ``Font``: where it sits in the atlas and how it is placed,
/// in em units.
public struct Glyph: Sendable, Equatable {
    /// The glyph's rectangle in the atlas, in texture coordinates: left, top,
    /// right, bottom.
    public var uv: SIMD4<Float>
    /// Its size, in em units.
    public var size: SIMD2<Float>
    /// From the pen position to its left edge, and up to its top edge, in em
    /// units.
    public var bearing: SIMD2<Float>
    /// How far the pen moves after it, in em units.
    public var advance: Float

    /// Creates a glyph.
    public init(uv: SIMD4<Float>, size: SIMD2<Float>, bearing: SIMD2<Float>, advance: Float) {
        self.uv = uv
        self.size = size
        self.bearing = bearing
        self.advance = advance
    }

    var c: StickShiftGlyph {
        StickShiftGlyph(
            u0: uv.x, v0: uv.y, u1: uv.z, v1: uv.w,
            width: size.x, height: size.y,
            bearing_x: bearing.x, bearing_y: bearing.y,
            advance: advance
        )
    }
}

/// A signed-distance-field font: an atlas texture and its glyph table. Keeps
/// its atlas alive; destroyed when you drop the last reference.
@MainActor
public final class Font {
    /// The context that owns it.
    public let context: Context
    /// The C handle's id.
    public let id: UInt64
    /// The atlas it draws from (linear, distance in red).
    public let atlas: Texture

    init(context: Context, id: UInt64, atlas: Texture) {
        self.context = context
        self.id = id
        self.atlas = atlas
    }

    /// The width of `text` at `pixelSize` pixels per em: the widest line, since
    /// `\n` starts a new one.
    public func width(of text: String, pixelSize: Float) throws -> Float {
        var width: Float = 0
        try check(stickshift_font_measure(context.raw, handle, text, pixelSize, &width))
        return width
    }

    var handle: StickShiftFont { StickShiftFont(id: id) }

    deinit {
        let id = id
        destroyResource(of: context) { stickshift_font_destroy($0, StickShiftFont(id: id)) }
    }
}

extension Context {
    /// Create a mesh from interleaved vertices and triangle indices.
    ///
    /// - Parameters:
    ///   - layout: How each vertex's floats are laid out.
    ///   - vertices: `layout.floatsPerVertex` floats per vertex.
    ///   - indices: Three per triangle, into `vertices`.
    /// - Throws: ``StickShiftError/Code-swift.struct/invalidArgument`` for
    ///   vertices that are not a whole number of `layout`'s vertices, indices
    ///   that are not whole triangles, or an index out of range.
    public func makeMesh(layout: VertexLayout, vertices: [Float], indices: [UInt32]) throws -> Mesh {
        var out = StickShiftMesh()
        try vertices.withUnsafeBytes { v throws in
            try indices.withUnsafeBytes { i throws in
                var desc = StickShiftMeshDesc(layout: layout.c, vertices: copied(v), indices: copied(i))
                try check(stickshift_mesh_create(raw, &desc, nil, &out))
            }
        }
        return Mesh(context: self, id: out.id, layout: layout)
    }

    /// Create a texture from RGBA8 pixels, row-major, top row first.
    ///
    /// - Throws: ``StickShiftError/Code-swift.struct/invalidArgument`` if
    ///   `rgba8` is not `width * height * 4` bytes, and
    ///   ``StickShiftError/Code-swift.struct/limitExceeded`` past 16384 pixels
    ///   a side.
    public func makeTexture(width: Int, height: Int, rgba8: [UInt8], colorSpace: ColorSpace = .srgb) throws
        -> Texture
    {
        guard let w = UInt32(exactly: width), let h = UInt32(exactly: height) else {
            throw StickShiftError(code: .invalidArgument, message: "width and height must fit in UInt32")
        }
        var out = StickShiftTexture()
        try rgba8.withUnsafeBytes { bytes throws in
            var desc = StickShiftTextureDesc(
                width: w, height: h, rgba8: copied(bytes), color_space: colorSpace.c, flags: 0
            )
            try check(stickshift_texture_create(raw, &desc, nil, &out))
        }
        return Texture(context: self, id: out.id, width: width, height: height, colorSpace: colorSpace)
    }

    /// Create a font over `atlas`: `glyphs[i]` is code point `firstCodepoint + i`,
    /// and `fallback` draws everything outside that range.
    ///
    /// - Throws: ``StickShiftError/Code-swift.struct/invalidArgument`` if the
    ///   atlas is not a ``ColorSpace/linear`` texture.
    public func makeFont(
        atlas: Texture,
        firstCodepoint: UInt32,
        glyphs: [Glyph],
        lineHeight: Float,
        fallback: Glyph
    ) throws -> Font {
        let table = glyphs.map(\.c)
        var out = StickShiftFont()
        try table.withUnsafeBufferPointer { g throws in
            var desc = StickShiftFontDesc(
                atlas: atlas.handle,
                first_codepoint: firstCodepoint,
                glyphs: g.baseAddress,
                glyph_count: UInt32(g.count),
                line_height: lineHeight,
                fallback: fallback.c
            )
            try check(stickshift_font_create(raw, &desc, nil, &out))
        }
        return Font(context: self, id: out.id, atlas: atlas)
    }
}
