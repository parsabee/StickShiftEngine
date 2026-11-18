// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

import Foundation
import OfframpGame

// `offramp`: the playable demo in Swift. With no options it plays in a window;
// `--smoke` and `--print-assets-dir` are the Rust binary's (`main.rs`), `--record`
// is `examples/record_demo.rs` and `--trace` is `examples/trace.rs`.
//
// The order is `main.rs`'s: parse the command line (help exits 0, a bad command
// line exits 2), then find the assets (exit 1 if there are none), then act.
//
// Stdout is line-buffered, as Rust's always is (`println!` writes through a
// `LineWriter`): C stdio would otherwise buffer whole blocks whenever stdout is
// a pipe or a file, so the game's "GAME OVER ..." and "replay ..." lines would
// come out late under `| tee`, and never if the process were killed.
setvbuf(stdout, nil, _IOLBF, 0)

/// Write `line` and a newline to stderr.
func printError(_ line: String) {
    FileHandle.standardError.write(Data((line + "\n").utf8))
}

/// Run `body` on the main actor, then exit: 0 when it returns, 1 with its error
/// on stderr when it throws.
func runToExit(_ body: @MainActor () throws -> Void) -> Never {
    MainActor.assumeIsolated {
        do {
            try body()
            exit(0)
        } catch {
            printError("error: \(error)")
            exit(1)
        }
    }
}

let command: OfframpCommand
switch parseArguments(Array(CommandLine.arguments.dropFirst())) {
case let .success(parsed):
    command = parsed
case let .failure(error):
    FileHandle.standardError.write(Data(error.description.utf8))
    exit(2)
}
if command == .help {
    print(OFFRAMP_HELP, terminator: "")
    exit(0)
}

// Find the assets once, up front, so a missing or misplaced directory is reported
// plainly instead of failing mid-boot.
let assets: String
switch AssetRoot.resolved {
case let .success(root):
    assets = root
case let .failure(error):
    printError("error: \(error)")
    exit(1)
}

switch command {
case .printAssetsDir:
    print(assets)
    exit(0)
case .smoke:
    runToExit { try runSmoke(frames: SMOKE_FRAMES) }
case let .trace(mode, summary):
    runToExit { try runTrace(mode, summary: summary) { print($0) } }
case let .record(directory, frames):
    runToExit { try record(to: directory, frames: frames) }
case .windowed:
    runWindowed()
case .help:
    exit(0)
}
