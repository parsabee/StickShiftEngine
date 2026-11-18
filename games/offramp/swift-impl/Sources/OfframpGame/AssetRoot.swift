// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

import Darwin
import Foundation

// Where the game's assets live at runtime, a port of
// `games/offramp/rust-impl/src/asset_root.rs` (spec 04 §2, contract decision 4).
//
// Offramp reads the Rust demo's committed assets in place (one copy). The root is
// resolved once, at start-up, as the first of these candidates that holds the
// marker file:
// 1. `$OFFRAMP_ASSETS`, if set and not empty. An explicit override is the only
//    candidate: if it does not hold the assets that is reported, never replaced;
// 2. `<executable dir>/../share/offramp/assets`, an installed layout (`bin/`
//    beside `share/`);
// 3. `<executable dir>/assets`, a flat layout;
// 4. the source tree's `games/offramp/shared/assets`, found from this file's
//    path at build time, so `swift run` and `swift test` work from a checkout.
// The executable's directory is tried resolved (symlinks followed) first, then
// as invoked.

/// Resolves the asset root.
public enum AssetRoot {
    /// The environment variable that overrides the root.
    public static let environmentVariable = "OFFRAMP_ASSETS"
    /// A file every asset root holds; a directory without it is skipped.
    public static let marker = "road.gltf"

    /// Nothing held the assets.
    public struct NotFound: Error, CustomStringConvertible, Equatable {
        /// The candidates, in the order tried.
        public let tried: [String]
        /// Whether the override was set (then it is the only candidate).
        public let environmentSet: Bool

        public var description: String {
            let env = AssetRoot.environmentVariable
            let marker = AssetRoot.marker
            if environmentSet {
                return "$\(env) is set to \(tried[0]), which does not hold the demo's assets "
                    + "(no \(marker) there). Point it at the assets directory, or unset it."
            }
            var text = "cannot find the demo's assets directory (one holding \(marker)). Tried, in order:\n"
            text += "  - $\(env) (not set)\n"
            for path in tried {
                text += "  - \(path)\n"
            }
            text += "Set \(env) to the directory holding the demo's assets (e.g. road.gltf)."
            return text
        }
    }

    /// The demo's crate directory in the source tree, the Rust
    /// `CARGO_MANIFEST_DIR` (`games/offramp/rust-impl`), from this file's path.
    public static let sourceTreeDirectory: String = {
        var url = URL(fileURLWithPath: #filePath)
        for _ in 0..<4 {
            url.deleteLastPathComponent()  // AssetRoot.swift, OfframpGame, Sources, swift-impl
        }
        return url.appendingPathComponent("rust-impl").path
    }()

    /// The candidate roots, in priority order, for override `environment` (empty
    /// counts as unset), executable path `executable` and source directory
    /// `manifestDirectory`.
    public static func candidates(environment: String?, executable: String?, manifestDirectory: String)
        -> [String]
    {
        if let env = environment, !env.isEmpty {
            return [env]
        }
        var out: [String] = []
        if let executable {
            // Resolved first: that finds an installed tree's share/ behind a symlink.
            var directories: [String] = []
            for path in [canonicalize(executable), executable] {
                if let path, let dir = parent(of: path), !directories.contains(dir) {
                    directories.append(dir)
                }
            }
            for dir in directories {
                out.append(join(dir, "../share/offramp/assets"))
            }
            for dir in directories {
                out.append(join(dir, "assets"))
            }
        }
        out.append(join(manifestDirectory, "../shared/assets"))
        return out
    }

    /// The first candidate holding the marker, canonicalised (or as written if
    /// that fails).
    public static func resolve(environment: String?, executable: String?, manifestDirectory: String)
        -> Result<String, NotFound>
    {
        let tried = candidates(
            environment: environment,
            executable: executable,
            manifestDirectory: manifestDirectory
        )
        if let found = tried.first(where: { isRegularFile(join($0, marker)) }) {
            return .success(canonicalize(found) ?? found)
        }
        return .failure(NotFound(tried: tried, environmentSet: !(environment ?? "").isEmpty))
    }

    /// The root resolved from the real environment, once.
    public static let resolved: Result<String, NotFound> = resolve(
        environment: ProcessInfo.processInfo.environment[environmentVariable],
        executable: executablePath(),
        manifestDirectory: sourceTreeDirectory
    )

    /// The resolved root. Traps with the error's text if there is none, which
    /// `main` has already ruled out.
    public static var root: String {
        switch resolved {
        case let .success(root): return root
        case let .failure(error): fatalError(error.description)
        }
    }

    /// The full path of asset `name`, relative to the root.
    public static func path(_ name: String) -> String {
        join(root, name)
    }

    // MARK: - Paths, as Rust's `std::path` treats them

    /// The running executable's path as the loader reports it
    /// (`_NSGetExecutablePath`, what Rust's `current_exe` reads), or `nil`.
    public static func executablePath() -> String? {
        var size: UInt32 = 0
        _ = _NSGetExecutablePath(nil, &size)
        var buffer = [CChar](repeating: 0, count: Int(size) + 1)
        guard _NSGetExecutablePath(&buffer, &size) == 0 else { return nil }
        let bytes = buffer.prefix { $0 != 0 }.map { UInt8(bitPattern: $0) }
        return String(decoding: bytes, as: UTF8.self)
    }

    /// `fs::canonicalize`: `realpath(3)`, which keeps `/private` (unlike
    /// `URL.resolvingSymlinksInPath`). `nil` if the path does not exist.
    public static func canonicalize(_ path: String) -> String? {
        guard let resolved = realpath(path, nil) else { return nil }
        defer { free(resolved) }
        return String(cString: resolved)
    }

    /// `Path::is_file`: the path, symlinks followed, is a regular file.
    public static func isRegularFile(_ path: String) -> Bool {
        var info = stat()
        guard stat(path, &info) == 0 else { return false }
        return (info.st_mode & S_IFMT) == S_IFREG
    }

    /// `Path::parent`: everything before the last component, `nil` for a root or
    /// an empty path.
    static func parent(of path: String) -> String? {
        var trimmed = Substring(path)
        while trimmed.count > 1 && trimmed.hasSuffix("/") {
            trimmed = trimmed.dropLast()
        }
        guard !trimmed.isEmpty, trimmed != "/" else { return nil }
        guard let slash = trimmed.lastIndex(of: "/") else { return "" }
        let head = trimmed[..<slash]
        return head.isEmpty ? "/" : String(head)
    }

    /// `Path::join` for a relative `name`: one `/` between, none doubled, and an
    /// empty directory adds nothing.
    static func join(_ directory: String, _ name: String) -> String {
        if directory.isEmpty {
            return name
        }
        return directory.hasSuffix("/") ? directory + name : directory + "/" + name
    }
}
