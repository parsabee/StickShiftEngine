// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

import Foundation
import StickShift
import simd

// Asset loading, a port of `games/playable-demo/src/assets.rs` (spec 04 §6-7):
// every model the demo draws, loaded once through the C API, and the fit
// transforms that place the cars and buildings on the road.
//
// The engine imports each file with the same importer the Rust demo uses, so the
// parts, their node matrices and their bounds are the same. Rust keeps per part
// `local = fit × node`; the C API keeps the nodes and multiplies
// `world × (local × node)` at draw time, so the game keeps `local = fit` and the
// product comes out in Rust's association, bit for bit (contract decision 2).

/// A model file that could not be loaded. Rust panics with these messages
/// (`assets.rs:50`, `:160`); the game treats it as fatal.
public struct AssetLoadError: Error, CustomStringConvertible {
    /// The file.
    public let path: String
    /// `"model"` for a multi-part model, `"textured"` for the road and skyline.
    public let kind: String
    /// The engine's error.
    public let underlying: any Error

    public var description: String {
        "import \(kind) asset \(path): \(underlying)"
    }
}

/// A model placed through a fit (Rust's `Vec<CarPart>` for one file): the model
/// and the transform that fits it to the road, with its matrix computed once,
/// as `assets.rs:61` does.
@MainActor
public struct FittedModel {
    /// The loaded model.
    public let model: Model
    /// The fit transform.
    public let fit: Transform
    /// `fit.to_matrix()`: the `local` every draw of this model passes.
    public let local: simd_float4x4

    /// Fits `model` with `fit`.
    public init(model: Model, fit: Transform) {
        self.model = model
        self.fit = fit
        local = fit.matrix
    }
}

/// `fit_transform` (`assets.rs:114-142`): the transform that fits a car model,
/// whose node-space bounds are `bounds`, to the road. A uniform scale makes its
/// longer horizontal side `CAR_LEN`; a `CAR_YAW` turn faces it down -Z; the
/// translation centres it in x and z with its underside at `CAR_BOTTOM`.
///
/// The bounds are `stickshift_model_get_bounds`, which are bit-identical to the
/// Rust fit's min and max over the same baked vertices (spec 04 §10.1).
public func fitTransform(min: SIMD3<Float>, max: SIMD3<Float>) -> Transform {
    let size = max - min
    let horizontal = RustF32.max(RustF32.max(size.x, size.z), RustF32.epsilon)
    let scale = CAR_LEN / horizontal
    let rotation = simd_quatf(glamRotationY: CAR_YAW)

    // Pivot: the footprint's centre in x and z, the underside in y.
    let pivot = SIMD3((min.x + max.x) * 0.5, min.y, (min.z + max.z) * 0.5)
    let turned = rotation.glamRotate(pivot * scale)
    let translation = SIMD3(-turned.x, -turned.y, -turned.z) + SIMD3(0, CAR_BOTTOM, 0)
    return Transform(position: translation, rotation: rotation, scale: SIMD3(repeating: scale))
}

/// `building_fit` (`assets.rs:84-107`): a uniform scale making the footprint
/// `BUILDING_FOOTPRINT`, centred in x and z with the underside at the origin, and
/// no turn.
public func buildingFit(min: SIMD3<Float>, max: SIMD3<Float>) -> Transform {
    let size = max - min
    let footprint = RustF32.max(RustF32.max(size.x, size.z), RustF32.epsilon)
    let scale = BUILDING_FOOTPRINT / footprint
    let pivot = SIMD3((min.x + max.x) * 0.5, min.y, (min.z + max.z) * 0.5)
    // `-(pivot * scale)`: a zero lane becomes -0.0, as in Rust.
    let scaled = pivot * scale
    return Transform(position: SIMD3(-scaled.x, -scaled.y, -scaled.z), scale: SIMD3(repeating: scale))
}

/// `pick_model` (`assets.rs:182-188`): `models[i]`, else the first, else nothing.
public func pickModel(_ models: [FittedModel], _ i: Int) -> FittedModel? {
    if i >= 0 && i < models.count {
        return models[i]
    }
    return models.first
}

/// Every model the demo draws (`game.rs:544-579`), loaded and fitted.
@MainActor
public struct DemoModels {
    /// The asphalt road tile (`ROAD_ASSET`): one textured quad, drawn per tile.
    public let road: Model
    /// The skyline backdrop (`SKYLINE_ASSET`): one textured, curved sheet.
    public let skyline: Model
    /// The player's car (`PLAYER_CAR_ASSET`), with `fitTransform`.
    public let playerCar: FittedModel
    /// The traffic cars, indexed like `TRAFFIC_CAR_ASSETS`, with `fitTransform`.
    public let trafficCars: [FittedModel]
    /// The building prefabs, indexed like `BUILDING_ASSETS`, with `buildingFit`.
    public let buildings: [FittedModel]

    /// Creates the set from loaded models.
    public init(road: Model, skyline: Model, playerCar: FittedModel, trafficCars: [FittedModel], buildings: [FittedModel]) {
        self.road = road
        self.skyline = skyline
        self.playerCar = playerCar
        self.trafficCars = trafficCars
        self.buildings = buildings
    }

    /// The model a placement draws (`render.rs`'s `model_parts`), or `nil` if
    /// there is none to pick from.
    public func model(for placed: PlacedModel) -> FittedModel? {
        switch placed {
        case .player: playerCar
        case let .traffic(i): pickModel(trafficCars, i)
        case let .building(i): pickModel(buildings, i)
        }
    }

    /// Load and fit every model through `renderer`, resolving each asset name
    /// with `assetPath`, before returning, as Rust's `init` does.
    ///
    /// The engine reads each file on a worker thread, so all thirteen are
    /// started first, in Rust's order (so the engine hands out the handles in
    /// that order), and then waited for in the same order. The order they
    /// finish in changes nothing that is drawn.
    ///
    /// - Throws: ``AssetLoadError`` for the first file, in that order, that
    ///   cannot be loaded. The other models are dropped.
    public static func load(_ renderer: Context, assetPath: (String) -> String) throws -> DemoModels {
        let textured = [ROAD_ASSET, SKYLINE_ASSET]
        let fitted = [PLAYER_CAR_ASSET] + TRAFFIC_CAR_ASSETS + BUILDING_ASSETS
        let jobs = (textured.map { ($0, "textured") } + fitted.map { ($0, "model") }).map { name, kind in
            (path: assetPath(name), kind: kind)
        }
        func failed(_ i: Int, _ error: any Error) -> AssetLoadError {
            AssetLoadError(path: jobs[i].path, kind: jobs[i].kind, underlying: error)
        }

        var models: [Model] = []
        for (i, job) in jobs.enumerated() {
            do {
                models.append(try renderer.startLoadingModel(contentsOf: URL(fileURLWithPath: job.path)))
            } catch {
                throw failed(i, error)
            }
        }
        for (i, model) in models.enumerated() {
            do {
                try model.wait()
            } catch {
                throw failed(i, error)
            }
        }

        func fit(_ i: Int, _ how: (SIMD3<Float>, SIMD3<Float>) -> Transform) throws -> FittedModel {
            let bounds: (min: SIMD3<Float>, max: SIMD3<Float>)
            do {
                bounds = try models[i].bounds()
            } catch {
                throw failed(i, error)
            }
            return FittedModel(model: models[i], fit: how(bounds.min, bounds.max))
        }
        let traffic = 3..<(3 + TRAFFIC_CAR_ASSETS.count)
        let buildings = traffic.upperBound..<(traffic.upperBound + BUILDING_ASSETS.count)
        return DemoModels(
            road: models[0],
            skyline: models[1],
            playerCar: try fit(2, fitTransform),
            trafficCars: try traffic.map { try fit($0, fitTransform) },
            buildings: try buildings.map { try fit($0, buildingFit) }
        )
    }
}
