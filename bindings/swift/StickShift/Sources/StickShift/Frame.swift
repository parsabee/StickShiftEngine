// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

import CStickShift
import simd

/// An open frame, handed to the closure of ``Context/frame(_:)``.
///
/// Everything recorded here is drawn when the closure returns. A frame is only
/// valid inside that closure: a call after it has been submitted throws
/// ``StickShiftError/Code-swift.struct/wrongState``.
///
/// The camera and the sun are **sticky**: they keep their last value across
/// frames until set again. Lights last **one frame**.
@MainActor
public struct Frame {
    let raw: OpaquePointer
    /// The context the frame belongs to.
    public let context: Context

    /// Set the camera the frame is seen from. Sticky across frames.
    public func setCamera(_ camera: Camera) throws {
        var c = StickShiftCamera(
            view_projection: StickShiftMat4(camera.viewProjection),
            position: StickShiftVec3(camera.position)
        )
        try check(stickshift_frame_set_camera(raw, &c))
    }

    /// Set the sun and sky. Sticky across frames.
    public func setSunSky(_ sun: SunSky) throws {
        var c = sun.c
        try check(stickshift_frame_set_sun_sky(raw, &c))
    }

    /// Add a point or spot light to this frame. Every light added before the
    /// frame is submitted lights every draw in it.
    ///
    /// - Throws: ``StickShiftError/Code-swift.struct/invalidArgument`` for a
    ///   light that cannot be drawn (see ``Light``), and
    ///   ``StickShiftError/Code-swift.struct/limitExceeded`` past
    ///   ``Light/maxPerFrame``.
    public func addLight(_ light: Light) throws {
        var c = light.c
        try check(stickshift_frame_add_light(raw, &c))
    }

    /// Draw `mesh` with model-to-world `transform` and `material`.
    ///
    /// - Returns: `false` if the mesh (or the material's texture) is not ready,
    ///   so nothing was drawn; `true` otherwise.
    @discardableResult
    public func draw(
        _ mesh: Mesh,
        transform: simd_float4x4 = matrix_identity_float4x4,
        material: Material = Material()
    ) throws -> Bool {
        var model = StickShiftMat4(transform)
        var m = StickShiftMaterial(
            base_color: StickShiftColor(material.baseColor),
            base_color_texture: material.texture?.handle ?? StickShiftTexture(id: 0),
            emissive: material.emissive.tuple
        )
        return try check(stickshift_frame_draw_mesh(raw, StickShiftMesh(id: mesh.id), &model, &m))
            == STICKSHIFT_OK
    }

    /// Draw every part of `model` at `transform`, its base colours multiplied
    /// by `tint`.
    ///
    /// - Returns: `false` if the model is not ready, so nothing was drawn.
    @discardableResult
    public func draw(_ model: Model, transform: simd_float4x4 = matrix_identity_float4x4, tint: SIMD4<Float> = .one)
        throws -> Bool
    {
        var world = StickShiftMat4(transform)
        var color = StickShiftColor(tint)
        return try check(stickshift_frame_draw_model(raw, StickShiftModel(id: model.id), &world, &color))
            == STICKSHIFT_OK
    }

    /// Draw the `parts` of `model`, in node order, each at
    /// `world × (local × node)`, where `node` is the part's node matrix from the
    /// glTF file, with its base colour multiplied by `tint`.
    ///
    /// A game that fits a model to its world (a scale, a turn, a lift) passes
    /// the fit as `local` and the entity's transform as `world`. The products
    /// are taken in that grouping, which can differ in the last bit from
    /// passing `world × local` to ``draw(_:transform:tint:)``.
    /// Drawing a model's ``ModelParts/textured`` and ``ModelParts/untextured``
    /// parts in two calls lets other draws go between them.
    ///
    /// - Returns: `false` if the model is not ready, so nothing was drawn.
    @discardableResult
    public func draw(
        _ model: Model,
        world: simd_float4x4,
        local: simd_float4x4 = matrix_identity_float4x4,
        parts: ModelParts = .all,
        tint: SIMD4<Float> = .one
    ) throws -> Bool {
        var draw = StickShiftModelDraw(
            world: StickShiftMat4(world),
            local: StickShiftMat4(local),
            tint: StickShiftColor(tint),
            parts: parts.c
        )
        return try check(stickshift_frame_draw_model_ex(raw, StickShiftModel(id: model.id), &draw))
            == STICKSHIFT_OK
    }

    /// Draw a 2D quad over the 3D scene. Quads composite in record order.
    @discardableResult
    public func draw(_ quad: Quad) throws -> Bool {
        var q = quad.c
        return try check(stickshift_frame_draw_quad(raw, &q)) == STICKSHIFT_OK
    }

    /// Draw `text` in `font` over the 3D scene, the pen starting at `origin` (the
    /// left end of the first line's baseline, in pixels from the top-left) at
    /// `pixelSize` pixels per em. `\n` starts a new line.
    ///
    /// - Returns: `false` if the font's atlas is gone, so nothing was drawn.
    @discardableResult
    public func drawText(
        _ text: String,
        font: Font,
        at origin: SIMD2<Float>,
        pixelSize: Float,
        color: SIMD4<Float> = .one
    ) throws -> Bool {
        try check(stickshift_frame_draw_text(raw, font.handle, text, origin.x, origin.y, pixelSize, StickShiftColor(color)))
            == STICKSHIFT_OK
    }

    /// Ask for this frame's pixels (offscreen contexts only). They arrive once
    /// the GPU has finished the frame: `await` ``PixelReadback/pixels()``, or
    /// call ``Context/flush()`` and read ``PixelReadback/bytes``.
    ///
    /// - Throws: ``StickShiftError/Code-swift.struct/unsupported`` on a layer
    ///   context, and ``StickShiftError/Code-swift.struct/wrongState`` for a
    ///   second request in the same frame.
    public func readPixels() throws -> PixelReadback {
        let (width, height) = context.size
        let readback = PixelReadback(width: width, height: height, context: context)
        let user = Unmanaged.passRetained(readback).toOpaque()
        var completion = StickShiftCompletion(callback: pixelsArrived, user: user)
        do {
            try check(
                stickshift_frame_read_pixels_async(
                    raw, readback.storage.baseAddress, readback.storage.count, &completion
                )
            )
        } catch {
            // The engine took nothing: no callback will come.
            Unmanaged<PixelReadback>.fromOpaque(user).release()
            throw error
        }
        context.pendingCompletions += 1
        context.pumpWhilePending()
        return readback
    }
}

/// The camera a frame is seen from.
public struct Camera: Sendable, Equatable {
    /// World to clip.
    public var viewProjection: simd_float4x4
    /// The eye, in world space: where specular highlights are seen from.
    public var position: SIMD3<Float>

    /// Creates a camera from its view-projection and the eye position it was
    /// built from.
    public init(viewProjection: simd_float4x4, position: SIMD3<Float>) {
        self.viewProjection = viewProjection
        self.position = position
    }

    /// A perspective camera at `eye` looking at `target`.
    ///
    /// - Parameters:
    ///   - fovY: The vertical field of view, in radians.
    ///   - aspect: Width over height.
    public static func perspective(
        eye: SIMD3<Float>,
        target: SIMD3<Float>,
        up: SIMD3<Float> = [0, 1, 0],
        fovY: Float,
        aspect: Float,
        near: Float = 0.1,
        far: Float = 300
    ) -> Camera {
        // Multiplied as the engine multiplies, not with simd's fused `*`, so
        // the camera matches one the Rust engine builds bit for bit.
        Camera(
            viewProjection: StickShiftMath.multiply(
                .perspective(fovY: fovY, aspect: aspect, near: near, far: far),
                .lookAt(eye: eye, target: target, up: up)
            ),
            position: eye
        )
    }
}

/// The sun and the sky it hangs in. Colours are linear RGB.
public struct SunSky: Sendable, Equatable {
    /// Height above the horizon in degrees: 0 on the horizon, 90 overhead.
    public var elevation: Float
    /// Bearing in degrees from -Z (north) toward +X (east).
    public var azimuth: Float
    /// The sunlight's colour.
    public var sunColor: SIMD3<Float>
    /// The direct light's strength. Zero at night.
    public var sunIntensity: Float
    /// Ambient fill from the sky: keep it well below `sunIntensity`.
    public var skyFill: Float
    /// The sky straight up.
    public var skyZenithColor: SIMD3<Float>
    /// The sky at the horizon; the fog fades toward it.
    public var skyHorizonColor: SIMD3<Float>

    /// The engine's default: a high white afternoon sun over a blue sky.
    public nonisolated static var `default`: SunSky {
        var c = StickShiftSunSky()
        stickshift_sun_sky_default(&c)
        return SunSky(
            elevation: c.sun_elevation_deg,
            azimuth: c.sun_azimuth_deg,
            sunColor: SIMD3(c.sun_color),
            sunIntensity: c.sun_intensity,
            skyFill: c.sky_fill,
            skyZenithColor: SIMD3(c.sky_zenith_color),
            skyHorizonColor: SIMD3(c.sky_horizon_color)
        )
    }

    /// Creates a sun and sky.
    public init(
        elevation: Float,
        azimuth: Float,
        sunColor: SIMD3<Float>,
        sunIntensity: Float,
        skyFill: Float,
        skyZenithColor: SIMD3<Float>,
        skyHorizonColor: SIMD3<Float>
    ) {
        self.elevation = elevation
        self.azimuth = azimuth
        self.sunColor = sunColor
        self.sunIntensity = sunIntensity
        self.skyFill = skyFill
        self.skyZenithColor = skyZenithColor
        self.skyHorizonColor = skyHorizonColor
    }

    var c: StickShiftSunSky {
        StickShiftSunSky(
            sun_elevation_deg: elevation,
            sun_azimuth_deg: azimuth,
            sun_color: sunColor.tuple,
            sun_intensity: sunIntensity,
            sky_fill: skyFill,
            sky_zenith_color: skyZenithColor.tuple,
            sky_horizon_color: skyHorizonColor.tuple
        )
    }
}

/// A point or spot light, added to one frame with ``Frame/addLight(_:)``.
///
/// Brightness falls off with the inverse square of the distance and reaches
/// exactly zero at ``range``; ``intensity`` is roughly the brightness one world
/// unit away, on the scale of ``SunSky/sunIntensity``. Keep ranges as short as
/// the light needs: the renderer shades a light only where it reaches.
public struct Light: Sendable, Equatable {
    /// The most lights one frame can hold.
    public static let maxPerFrame = Int(STICKSHIFT_MAX_LIGHTS)

    /// Point or spot.
    public enum Kind: Sendable, Equatable {
        /// Shines equally in every direction.
        case point
        /// Shines in a cone along `direction` (the way the light travels): at
        /// full strength inside `innerAngle` degrees of the axis, gone past
        /// `outerAngle` degrees (`0 <= innerAngle <= outerAngle <= 90`).
        case spot(direction: SIMD3<Float>, innerAngle: Float, outerAngle: Float)
    }

    /// Point or spot, and a spot's cone.
    public var kind: Kind
    /// World-space position.
    public var position: SIMD3<Float>
    /// Linear RGB colour.
    public var color: SIMD3<Float>
    /// Brightness, `>= 0`.
    public var intensity: Float
    /// Distance at which the light has faded to zero, `> 0`.
    public var range: Float

    /// Creates a light.
    public init(kind: Kind, position: SIMD3<Float>, color: SIMD3<Float>, intensity: Float, range: Float) {
        self.kind = kind
        self.position = position
        self.color = color
        self.intensity = intensity
        self.range = range
    }

    /// A point light.
    public static func point(at position: SIMD3<Float>, color: SIMD3<Float> = .one, intensity: Float, range: Float)
        -> Light
    {
        Light(kind: .point, position: position, color: color, intensity: intensity, range: range)
    }

    /// A spot light at `position` shining along `direction`.
    ///
    /// - Parameters:
    ///   - innerAngle: Half angle in degrees inside which it is at full strength.
    ///   - outerAngle: Half angle in degrees past which it is gone.
    public static func spot(
        at position: SIMD3<Float>,
        direction: SIMD3<Float>,
        color: SIMD3<Float> = .one,
        intensity: Float,
        range: Float,
        innerAngle: Float,
        outerAngle: Float
    ) -> Light {
        Light(
            kind: .spot(direction: direction, innerAngle: innerAngle, outerAngle: outerAngle),
            position: position,
            color: color,
            intensity: intensity,
            range: range
        )
    }

    var c: StickShiftLight {
        var light = StickShiftLight()
        light.position = StickShiftVec3(position)
        light.color = color.tuple
        light.intensity = intensity
        light.range = range
        switch kind {
        case .point:
            light.kind = StickShiftLightKind(STICKSHIFT_LIGHT_POINT)
        case let .spot(direction, inner, outer):
            light.kind = StickShiftLightKind(STICKSHIFT_LIGHT_SPOT)
            light.direction = StickShiftVec3(direction)
            light.inner_angle_deg = inner
            light.outer_angle_deg = outer
        }
        return light
    }
}

/// Which parts of a ``Model`` a draw records: see
/// ``Frame/draw(_:world:local:parts:tint:)``.
public enum ModelParts: Sendable {
    /// Every part.
    case all
    /// The parts with a base-colour texture
    /// (``VertexLayout/positionNormalUV``).
    case textured
    /// The parts without one (``VertexLayout/positionNormalColor``), coloured
    /// by their vertices.
    case untextured

    var c: StickShiftModelParts {
        switch self {
        case .all: StickShiftModelParts(STICKSHIFT_MODEL_PARTS_ALL)
        case .textured: StickShiftModelParts(STICKSHIFT_MODEL_PARTS_TEXTURED)
        case .untextured: StickShiftModelParts(STICKSHIFT_MODEL_PARTS_UNTEXTURED)
        }
    }
}

/// How a mesh draw looks. Colours are linear.
@MainActor
public struct Material {
    /// Multiplies the base-colour texture (``VertexLayout/positionNormalUV``
    /// meshes; vertex-colour meshes take their colour from their vertices).
    public var baseColor: SIMD4<Float>
    /// The base-colour texture for ``VertexLayout/positionNormalUV`` meshes;
    /// `nil` for white.
    public var texture: Texture?
    /// Light the surface gives off itself, added after lighting: a lit window
    /// or a lamp glows without lighting anything else. May exceed 1.
    public var emissive: SIMD3<Float>

    /// Creates a material.
    public init(baseColor: SIMD4<Float> = .one, texture: Texture? = nil, emissive: SIMD3<Float> = .zero) {
        self.baseColor = baseColor
        self.texture = texture
        self.emissive = emissive
    }
}

/// A 2D quad over the 3D scene, in drawable pixels from the top-left.
@MainActor
public struct Quad {
    /// How a quad is filled.
    public enum Fill {
        /// Its colour.
        case solid
        /// A texture's colours, times its colour.
        case texture(Texture)
        /// A signed-distance field (distance in red) tinted by its colour: how
        /// text is drawn. The texture must be ``ColorSpace/linear``.
        case sdf(Texture)
    }

    /// Left, top, width, height, in pixels.
    public var rect: SIMD4<Float>
    /// The texture rectangle: left, top, right, bottom.
    public var uv: SIMD4<Float>
    /// The tint; alpha blends.
    public var color: SIMD4<Float>
    /// How it is filled.
    public var fill: Fill

    /// Creates a quad.
    public init(rect: SIMD4<Float>, color: SIMD4<Float> = .one, fill: Fill = .solid, uv: SIMD4<Float> = [0, 0, 1, 1]) {
        self.rect = rect
        self.uv = uv
        self.color = color
        self.fill = fill
    }

    var c: StickShiftQuad {
        let (fill, texture): (Int32, StickShiftTexture) =
            switch self.fill {
            case .solid: (STICKSHIFT_FILL_SOLID, StickShiftTexture(id: 0))
            case let .texture(t): (STICKSHIFT_FILL_TEXTURE, t.handle)
            case let .sdf(t): (STICKSHIFT_FILL_SDF, t.handle)
            }
        return StickShiftQuad(
            x: rect.x, y: rect.y, width: rect.z, height: rect.w,
            u0: uv.x, v0: uv.y, u1: uv.z, v1: uv.w,
            color: StickShiftColor(color),
            fill: StickShiftFill(fill),
            texture: texture
        )
    }
}

/// A frame's pixels on their way back from the GPU, from ``Frame/readPixels()``.
///
/// Its buffer is the one the engine writes into, so it stays alive until the
/// engine is done with it, even if you drop it sooner.
public final class PixelReadback: @unchecked Sendable {
    /// Where the read-back is.
    public enum State: Sendable, Equatable {
        /// The frame has not finished.
        case pending
        /// The pixels are in ``PixelReadback/bytes``.
        case complete
        /// They will never arrive (the frame was skipped or discarded).
        case failed(StickShiftError)
    }

    /// Width, in pixels.
    public let width: Int
    /// Height, in pixels.
    public let height: Int
    let storage: UnsafeMutableRawBufferPointer
    private let context: Context
    // Written only inside a poll on the context's thread.
    private(set) var current: State = .pending
    private var waiters: [CheckedContinuation<[UInt8], any Error>] = []

    init(width: Int, height: Int, context: Context) {
        self.width = width
        self.height = height
        self.context = context
        storage = .allocate(byteCount: width * height * 4, alignment: 16)
        storage.initializeMemory(as: UInt8.self, repeating: 0)
    }

    deinit {
        storage.deallocate()
    }

    /// Where the read-back is.
    @MainActor
    public var state: State { current }

    /// The pixels once ``state`` is ``State/complete``: BGRA8, `width * 4` bytes
    /// per row, top row first. `nil` before then.
    @MainActor
    public var bytes: [UInt8]? {
        current == .complete ? Array(storage) : nil
    }

    /// Wait for the pixels: BGRA8, `width * 4` bytes per row, top row first.
    @MainActor
    public func pixels() async throws -> [UInt8] {
        switch current {
        case .complete: return Array(storage)
        case let .failed(error): throw error
        case .pending:
            return try await withCheckedThrowingContinuation { waiters.append($0) }
        }
    }

    func finish(_ state: State) {
        current = state
        let bytes = Array(storage)
        for waiter in waiters {
            switch state {
            case .complete: waiter.resume(returning: bytes)
            case let .failed(error): waiter.resume(throwing: error)
            case .pending: break
            }
        }
        waiters = []
        MainActor.assumeIsolated {
            context.pendingCompletions -= 1
        }
    }
}

/// The completion for a read-back.
private let pixelsArrived: StickShiftCompletionFn = { user, event in
    guard let user, let event else { return }
    let readback = Unmanaged<PixelReadback>.fromOpaque(user).takeRetainedValue()
    let e = event.pointee
    if Int32(bitPattern: e.kind) == STICKSHIFT_EVENT_READBACK_COMPLETE {
        readback.finish(.complete)
    } else {
        readback.finish(
            .failed(
                StickShiftError(
                    code: StickShiftError.Code(rawValue: e.result),
                    message: e.message.map { String(cString: $0) } ?? ""
                )
            )
        )
    }
}
