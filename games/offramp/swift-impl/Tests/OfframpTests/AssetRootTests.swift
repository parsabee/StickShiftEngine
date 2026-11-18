// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

import Foundation
import Testing

@testable import OfframpGame

/// A fresh, empty, canonical temp directory, removed when dropped.
private final class TempDir {
    let path: String

    init() throws {
        let dir = (NSTemporaryDirectory() as NSString)
            .appendingPathComponent("offramp-asset-root-\(getpid())-\(UUID().uuidString)")
        try? FileManager.default.removeItem(atPath: dir)
        try FileManager.default.createDirectory(atPath: dir, withIntermediateDirectories: true)
        // Canonical, so comparisons survive macOS's /var -> /private/var link.
        path = try #require(AssetRoot.canonicalize(dir))
    }

    deinit {
        try? FileManager.default.removeItem(atPath: path)
    }

    func join(_ rel: String) -> String {
        path + "/" + rel
    }

    @discardableResult
    func mkdir(_ rel: String) throws -> String {
        let p = join(rel)
        try FileManager.default.createDirectory(atPath: p, withIntermediateDirectories: true)
        return p
    }

    /// A directory that holds the assets (has the marker file).
    @discardableResult
    func assets(_ rel: String) throws -> String {
        let p = try mkdir(rel)
        FileManager.default.createFile(atPath: p + "/" + AssetRoot.marker, contents: Data())
        return p
    }

    @discardableResult
    func touch(_ rel: String) throws -> String {
        let p = join(rel)
        try FileManager.default.createDirectory(
            atPath: (p as NSString).deletingLastPathComponent,
            withIntermediateDirectories: true
        )
        FileManager.default.createFile(atPath: p, contents: Data())
        return p
    }
}

private func same(_ a: String, _ b: String) -> Bool {
    AssetRoot.canonicalize(a) != nil && AssetRoot.canonicalize(a) == AssetRoot.canonicalize(b)
}

private func resolve(_ env: String?, _ exe: String?, _ manifest: String) -> Result<String, AssetRoot.NotFound> {
    AssetRoot.resolve(environment: env, executable: exe, manifestDirectory: manifest)
}

/// Ports of `games/offramp/rust-impl/src/asset_root.rs`'s tests, with Offramp's
/// variable (`OFFRAMP_ASSETS`) and install layout (`share/offramp/assets`).
@Suite("asset root")
struct AssetRootTests {
    @Test("env_var_wins_over_everything") func envVarWinsOverEverything() throws {
        let t = try TempDir()
        let env = try t.assets("custom")
        let exe = try t.touch("prefix/bin/offramp")
        try t.assets("prefix/share/offramp/assets")
        try t.assets("prefix/bin/assets")
        let manifest = try t.mkdir("src")
        try t.assets("shared/assets")

        let root = try resolve(env, exe, manifest).get()
        #expect(same(root, env))
    }

    @Test("unset_or_empty_env_var_falls_through_to_share_layout")
    func unsetOrEmptyEnvVarFallsThroughToShareLayout() throws {
        let t = try TempDir()
        let exe = try t.touch("prefix/bin/offramp")
        let share = try t.assets("prefix/share/offramp/assets")
        try t.assets("prefix/bin/assets")
        let manifest = try t.mkdir("src")
        try t.assets("shared/assets")

        for env in ["", nil] as [String?] {
            let root = try resolve(env, exe, manifest).get()
            #expect(same(root, share), "env \(String(describing: env)) resolved to \(root)")
        }
    }

    @Test("a_wrong_env_var_is_reported_not_replaced") func aWrongEnvVarIsReportedNotReplaced() throws {
        let t = try TempDir()
        let exe = try t.touch("prefix/bin/offramp")
        try t.assets("prefix/share/offramp/assets")
        let manifest = try t.mkdir("src")
        let empty = try t.mkdir("empty")
        for env in [t.join("nope"), empty] {
            guard case let .failure(error) = resolve(env, exe, manifest) else {
                Issue.record("\(env) resolved")
                continue
            }
            #expect(error.description.contains(env), "\(error)")
            #expect(error.description.contains(AssetRoot.marker), "\(error)")
        }
    }

    @Test("a_directory_without_the_marker_is_skipped") func aDirectoryWithoutTheMarkerIsSkipped() throws {
        let t = try TempDir()
        let exe = try t.touch("flat/offramp")
        try t.mkdir("flat/assets")
        let manifest = try t.mkdir("src")
        let srcAssets = try t.assets("shared/assets")
        #expect(same(try resolve(nil, exe, manifest).get(), srcAssets))
    }

    @Test("flat_layout_beats_the_build_dir") func flatLayoutBeatsTheBuildDir() throws {
        let t = try TempDir()
        let exe = try t.touch("flat/offramp")
        let flat = try t.assets("flat/assets")
        let manifest = try t.mkdir("src")
        try t.assets("shared/assets")
        #expect(same(try resolve(nil, exe, manifest).get(), flat))
    }

    @Test("build_dir_is_the_last_resort") func buildDirIsTheLastResort() throws {
        let t = try TempDir()
        let exe = try t.touch("bin/offramp")
        let manifest = try t.mkdir("src")
        let srcAssets = try t.assets("shared/assets")
        #expect(same(try resolve(nil, exe, manifest).get(), srcAssets))
        #expect(same(try resolve(nil, nil, manifest).get(), srcAssets))
    }

    @Test("symlinked_binary_finds_the_keg_share_dir") func symlinkedBinaryFindsTheKegShareDir() throws {
        let t = try TempDir()
        let real = try t.touch("Cellar/offramp/1.0/bin/offramp")
        let kegAssets = try t.assets("Cellar/offramp/1.0/share/offramp/assets")
        try t.mkdir("prefix/bin")
        let link = t.join("prefix/bin/offramp")
        try FileManager.default.createSymbolicLink(atPath: link, withDestinationPath: real)
        let manifest = try t.mkdir("src")
        #expect(same(try resolve(nil, link, manifest).get(), kegAssets))
    }

    @Test("nothing_found_lists_every_candidate") func nothingFoundListsEveryCandidate() throws {
        let t = try TempDir()
        let exe = try t.touch("bin/offramp")
        let manifest = t.join("gone")

        guard case let .failure(error) = resolve(nil, exe, manifest) else {
            Issue.record("resolved with nothing there")
            return
        }
        let msg = error.description
        #expect(msg.contains("$OFFRAMP_ASSETS (not set)"), "\(msg)")
        #expect(msg.contains("share/offramp/assets"), "\(msg)")
        #expect(msg.contains(manifest + "/../shared/assets"), "\(msg)")
        #expect(error.tried.count == 3, "share, flat, build dir: \(msg)")

        let env = t.join("custom")
        guard case let .failure(override) = resolve(env, exe, manifest) else {
            Issue.record("resolved a missing override")
            return
        }
        #expect(!override.description.contains("not set"), "\(override)")
        #expect(override.description.contains(env), "\(override)")
        #expect(override.tried == [env], "an explicit override is the only candidate")
    }

    @Test func theMessagesKeepTheRustShapes() {
        let set = AssetRoot.NotFound(tried: ["/nonexistent"], environmentSet: true)
        #expect(
            set.description
                == "$OFFRAMP_ASSETS is set to /nonexistent, which does not hold the demo's assets "
                + "(no road.gltf there). Point it at the assets directory, or unset it."
        )
        let unset = AssetRoot.NotFound(tried: ["/a/../share/offramp/assets", "/a/assets", "/src/assets"], environmentSet: false)
        #expect(
            unset.description == """
                cannot find the demo's assets directory (one holding road.gltf). Tried, in order:
                  - $OFFRAMP_ASSETS (not set)
                  - /a/../share/offramp/assets
                  - /a/assets
                  - /src/assets
                Set OFFRAMP_ASSETS to the directory holding the demo's assets (e.g. road.gltf).
                """,
            "the candidates print as written, `..` kept; no newline at the end"
        )
    }

    @Test func candidatesKeepTheRustOrderAndSpelling() throws {
        let t = try TempDir()
        let real = try t.touch("real/bin/offramp")
        try t.mkdir("link/bin")
        let link = t.join("link/bin/offramp")
        try FileManager.default.createSymbolicLink(atPath: link, withDestinationPath: real)
        #expect(
            AssetRoot.candidates(environment: nil, executable: link, manifestDirectory: "/m") == [
                t.join("real/bin/../share/offramp/assets"),
                t.join("link/bin/../share/offramp/assets"),
                t.join("real/bin/assets"),
                t.join("link/bin/assets"),
                "/m/../shared/assets",
            ],
            "the resolved directory first, then the invoked one"
        )
        #expect(
            AssetRoot.candidates(environment: nil, executable: real, manifestDirectory: "/m").count == 3,
            "an unlinked executable's two directories are one"
        )
        #expect(AssetRoot.candidates(environment: "/e", executable: real, manifestDirectory: "/m") == ["/e"])
        #expect(AssetRoot.candidates(environment: "", executable: nil, manifestDirectory: "/m") == ["/m/../shared/assets"])
    }

    @Test func onlyARegularFileIsTheMarker() throws {
        let t = try TempDir()
        try t.mkdir("dir/road.gltf")
        #expect(!AssetRoot.isRegularFile(t.join("dir/road.gltf")), "a directory named like the marker is not it")
        let file = try t.touch("file/road.gltf")
        #expect(AssetRoot.isRegularFile(file))
        try FileManager.default.createSymbolicLink(atPath: t.join("viaLink"), withDestinationPath: file)
        #expect(AssetRoot.isRegularFile(t.join("viaLink")), "symlinks are followed")
        #expect(!AssetRoot.isRegularFile(t.join("missing")))
    }

    @Test func pathHelpersFollowRustsPath() {
        #expect(AssetRoot.parent(of: "/a/b/exe") == "/a/b")
        #expect(AssetRoot.parent(of: "/exe") == "/")
        #expect(AssetRoot.parent(of: "exe") == "")
        #expect(AssetRoot.parent(of: "/") == nil)
        #expect(AssetRoot.parent(of: "/a/b/") == "/a")
        #expect(AssetRoot.join("/a", "b") == "/a/b")
        #expect(AssetRoot.join("/", "b") == "/b")
        #expect(AssetRoot.join("", "b") == "b")
        #expect(AssetRoot.canonicalize("/private/tmp/../tmp") == "/private/tmp")
        #expect(AssetRoot.canonicalize("/tmp") == "/private/tmp", "realpath keeps /private")
    }

    @Test("the_real_root_holds_the_demo_assets") func theRealRootHoldsTheDemoAssets() throws {
        // Under `swift test` the root resolves to the checkout's demo assets
        // (unless OFFRAMP_ASSETS points elsewhere), and every path joins onto it.
        let root = try AssetRoot.resolved.get()
        #expect(AssetRoot.isRegularFile(root + "/road.gltf"))
        #expect(AssetRoot.path("font.bin").hasSuffix("font.bin"))
        #expect(AssetRoot.path("font.bin") == root + "/font.bin")
        if ProcessInfo.processInfo.environment[AssetRoot.environmentVariable] == nil {
            #expect(same(root, demoAssets.path), "the source tree's games/offramp/shared/assets")
        }
        #expect(AssetRoot.sourceTreeDirectory.hasSuffix("/games/offramp/rust-impl"))
        #expect(!AssetRoot.sourceTreeDirectory.contains(".."))
    }
}
