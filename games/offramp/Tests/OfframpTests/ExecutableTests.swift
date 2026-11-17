// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

import Foundation
import Testing

@testable import OfframpGame

// The `offramp` executable itself, run as a process: what it prints and how it
// exits, against the Rust binary's goldens (spec 02 §5, 04 §1). `swift test`
// builds the executable with the tests; `scripts/offramp-parity.sh` also runs
// both binaries side by side.

/// What a run of `offramp` printed, and its exit status.
private struct Run {
    let status: Int32
    let stdout: Data
    let stderr: Data

    var out: String { String(decoding: stdout, as: UTF8.self) }
    var err: String { String(decoding: stderr, as: UTF8.self) }
}

/// A class of this test bundle, to find the bundle by.
private final class BundleToken: NSObject {}

/// The executable built with these tests: beside the test bundle, in whatever
/// build directory and configuration built it (`.build/debug`, `swift test -c
/// release`, `--scratch-path`, Xcode's DerivedData).
private let executable = Bundle(for: BundleToken.self).bundleURL
    .deletingLastPathComponent()
    .appendingPathComponent("offramp")

/// A started `offramp` process, with its stdout and stderr going to files.
private struct Started {
    let process: Process
    let scratch: URL
    let outURL: URL
    let errURL: URL
    let out: FileHandle
    let err: FileHandle

    /// Wait for it to exit and collect what it printed.
    func finish() throws -> Run {
        process.waitUntilExit()
        try out.close()
        try err.close()
        defer { try? FileManager.default.removeItem(at: scratch) }
        return Run(status: process.terminationStatus, stdout: try Data(contentsOf: outURL), stderr: try Data(contentsOf: errURL))
    }
}

/// Start `offramp` with `arguments`, and `environment` on top of this process's.
private func start(_ arguments: [String], environment: [String: String] = [:]) throws -> Started {
    try #require(
        FileManager.default.isExecutableFile(atPath: executable.path),
        "the offramp executable is built with the tests, beside the test bundle, at \(executable.path)"
    )
    let scratch = FileManager.default.temporaryDirectory.appendingPathComponent("offramp-run-\(UUID().uuidString)")
    try FileManager.default.createDirectory(at: scratch, withIntermediateDirectories: true)
    let outURL = scratch.appendingPathComponent("stdout")
    let errURL = scratch.appendingPathComponent("stderr")
    FileManager.default.createFile(atPath: outURL.path, contents: nil)
    FileManager.default.createFile(atPath: errURL.path, contents: nil)

    let process = Process()
    process.executableURL = executable
    process.arguments = arguments
    // In the scratch directory, which `finish()` removes, so a relative path the
    // run writes to never lands in the package (`swift test` runs in its root).
    process.currentDirectoryURL = scratch
    var env = ProcessInfo.processInfo.environment
    env.removeValue(forKey: AssetRoot.environmentVariable)
    env.merge(environment) { $1 }
    process.environment = env
    let out = try FileHandle(forWritingTo: outURL)
    let err = try FileHandle(forWritingTo: errURL)
    process.standardOutput = out
    process.standardError = err
    try process.run()
    return Started(process: process, scratch: scratch, outURL: outURL, errURL: errURL, out: out, err: err)
}

/// Run `offramp` with `arguments`, and `environment` on top of this process's.
private func offramp(_ arguments: String..., environment: [String: String] = [:]) throws -> Run {
    try start(arguments, environment: environment).finish()
}

@Suite("the offramp executable")
struct ExecutableTests {
    @Test func smokeIsTheRustOutputByteForByte() throws {
        let run = try offramp("--smoke")
        #expect(run.status == 0)
        // Golden: `cargo run -p playable-demo -- --smoke` (spec 02 §5.3).
        let golden = "GAME OVER \u{2014} score 23 \u{2014} best 23. Press Space to replay.\nsmoke: 120 frames OK\n"
        #expect(run.stdout == Data(golden.utf8))
        #expect(run.stderr.isEmpty)
        // `--` at the end changes nothing, as in clap.
        let terminated = try offramp("--smoke", "--")
        #expect(terminated.status == 0)
        #expect(terminated.stdout == Data(golden.utf8))
        #expect(terminated.stderr.isEmpty)
    }

    @Test func printAssetsDirPrintsTheCanonicalRoot() throws {
        let run = try offramp("--print-assets-dir")
        #expect(run.status == 0)
        let root = try #require(AssetRoot.canonicalize(demoAssets.path))
        #expect(run.out == root + "\n")
        #expect(run.stderr.isEmpty)
        // It wins over --smoke, as in Rust.
        #expect(try offramp("--smoke", "--print-assets-dir").out == root + "\n")
    }

    @Test func missingAssetsExitOneBeforeAnythingRuns() throws {
        let run = try offramp("--smoke", environment: [AssetRoot.environmentVariable: "/nonexistent"])
        #expect(run.status == 1)
        #expect(run.stdout.isEmpty)
        #expect(
            run.err
                == "error: $OFFRAMP_ASSETS is set to /nonexistent, which does not hold the demo's assets (no road.gltf there). Point it at the assets directory, or unset it.\n"
        )
    }

    @Test func aBadCommandLineExitsTwoBeforeTheAssetsAreLookedFor() throws {
        let run = try offramp("--bogus", environment: [AssetRoot.environmentVariable: "/nonexistent"])
        #expect(run.status == 2)
        #expect(run.stdout.isEmpty)
        #expect(run.err == UsageError(message: "unexpected argument '--bogus' found").description)
        #expect(try offramp("--trace", "fast").status == 2)
        #expect(try offramp("--frames", "3").status == 2)
        // A flag is never an option's value: this records nothing, not even into
        // a directory called `--smoke` where it runs.
        let started = try start(["--record", "--smoke"])
        started.process.waitUntilExit()
        let written = try FileManager.default.contentsOfDirectory(atPath: started.scratch.path).sorted()
        let flagAsValue = try started.finish()
        #expect(written == ["stderr", "stdout"], "nothing written where it ran")
        #expect(flagAsValue.status == 2)
        #expect(flagAsValue.stdout.isEmpty)
        #expect(flagAsValue.err == UsageError(message: "a value is required for '--record <DIR>' but none was supplied").description)
    }

    @Test func helpExitsZero() throws {
        let run = try offramp("--help")
        #expect(run.status == 0)
        #expect(run.out == OFFRAMP_HELP)
        #expect(run.stderr.isEmpty)
        // A cluster of short flags starting with `-h` is the help, as in clap.
        let cluster = try offramp("-hh")
        #expect(cluster.status == 0)
        #expect(cluster.out == OFFRAMP_HELP)
        #expect(cluster.stderr.isEmpty)
        // Before the assets are looked for, as clap exits in Rust: with none to
        // be found, the help still prints and exits 0.
        let noAssets = try offramp("--help", environment: [AssetRoot.environmentVariable: "/nonexistent"])
        #expect(noAssets.status == 0)
        #expect(noAssets.out == OFFRAMP_HELP)
        #expect(noAssets.stderr.isEmpty)
    }

    @Test func aFailedRunPrintsItsErrorAndExitsOne() throws {
        // An asset directory that passes the lookup (it has the `road.gltf`
        // marker) but holds no real models: `initialize` throws on a model's
        // import, and the run prints that error and exits 1, with nothing on
        // stdout (not the smoke run's OK line).
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent("offramp-assets-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: directory) }
        FileManager.default.createFile(atPath: directory.appendingPathComponent("road.gltf").path, contents: Data())

        for arguments in [["--smoke"], ["--trace", "smoke"]] {
            let run = try start(arguments, environment: [AssetRoot.environmentVariable: directory.path]).finish()
            #expect(run.status == 1, "\(arguments)")
            #expect(run.stdout.isEmpty, "\(arguments)")
            let lines = run.err.split(separator: "\n", omittingEmptySubsequences: false)
            #expect(lines.last == "", "\(arguments): stderr ends with a newline")
            let errors = lines.filter { $0.hasPrefix("error: ") }
            #expect(errors.count == 1, "\(arguments): one error line: \(run.err)")
            #expect(errors.first?.contains(".gltf") == true, "\(arguments): a model's import fails: \(run.err)")
            #expect(lines.dropLast().last?.hasPrefix("error: ") == true, "\(arguments): nothing after the error")
        }
    }

    @Test func stdoutIsLineBufferedIntoAFile() throws {
        // Rust's stdout is always line-buffered, so a line the game prints is in
        // a file or pipe the moment it is printed, even if the process is then
        // killed. Block-buffered output would reach the file in whole buffers
        // that end mid-line, and the rest would be lost on the kill.
        let run = try start(["--trace", "long"])
        func printed() -> Int {
            ((try? FileManager.default.attributesOfItem(atPath: run.outURL.path))?[.size] as? Int) ?? 0
        }
        let deadline = Date(timeIntervalSinceNow: 60)
        while printed() == 0, run.process.isRunning, Date() < deadline {
            Thread.sleep(forTimeInterval: 0.01)
        }
        let stillRunning = run.process.isRunning
        run.process.terminate()
        let result = try run.finish()
        try #require(stillRunning, "the long trace was still printing when it was stopped")
        #expect(run.process.terminationReason == .uncaughtSignal)
        #expect(!result.stdout.isEmpty)
        #expect(result.stdout.last == UInt8(ascii: "\n"), "what reached the file ends at a line's end")
    }

    @Test func theSmokeTraceIsTheRustTrace() throws {
        let run = try offramp("--trace", "smoke")
        #expect(run.status == 0)
        #expect(run.stderr.isEmpty)
        // `cargo run -p playable-demo --example trace -- smoke`.
        let rust = try Data(contentsOf: fixturesDirectory.appendingPathComponent("trace_smoke.jsonl"))
        #expect(run.stdout == rust)
    }
}
