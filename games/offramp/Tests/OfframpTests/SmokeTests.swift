// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

import Testing

@testable import OfframpGame

/// `main.rs`'s tests of the smoke oracle, on the `runSmoke` the executable runs.
@MainActor
@Suite("smoke")
struct SmokeTests {
    @Test("smoke_runs_without_panicking") func smokeRunsWithoutPanicking() throws {
        // The oracle completes the fixed frame count without a GPU.
        var lines: [String] = []
        try runSmoke(frames: SMOKE_FRAMES) { lines.append($0) }

        // Golden: the Rust demo's `--smoke` stdout, byte for byte: the game's own
        // report from frame 99's update, then the contract line.
        try #require(lines == ["GAME OVER — score 23 — best 23. Press Space to replay.", "smoke: 120 frames OK"])
        #expect(Array(lines[0].utf8)[10...12] == [0xE2, 0x80, 0x94], "an em dash, U+2014")
    }

    @Test("smoke_prints_the_expected_contract_line") func smokePrintsTheExpectedContractLine() {
        // Guards the exact `--smoke` stdout contract.
        #expect(smokeReport(SMOKE_FRAMES) == "smoke: 120 frames OK")
    }
}
