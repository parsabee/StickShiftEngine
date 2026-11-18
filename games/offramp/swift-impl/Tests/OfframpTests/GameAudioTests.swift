// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

import Foundation
import StickShift
import Testing

@testable import OfframpGame

/// `game.rs`'s audio tests, one to one, with their names and messages: what the
/// game asks of the engine's audio, read from its counters. The headless engine
/// is silent, so it counts every request and plays nothing. Where the Rust probe
/// captured the values a test reaches, they are checked too ("golden").
@MainActor
@Suite("game audio")
struct GameAudioTests {
    @Test("sounds_load_once_in_init_and_the_title_screen_stays_silent")
    func soundsLoadOnceInInitAndTheTitleScreenStaysSilent() throws {
        // Audio is loaded at init and only played on a game event.
        let game = CarRunner()
        let h = try Headless(game)
        try h.run(frames: 30)

        // Headless means no audio device at all.
        let title = h.audio.stats
        #expect(title.isSilent, "a headless run opens no audio device")
        #expect(title.soundCount == 2, "the music and the impact, loaded once each")
        #expect(title.decodeCount == 0, "silent mode decodes nothing")
        #expect(game.music != nil && game.impact != nil, "both loaded in init")
        #expect(game.music != game.impact, "two distinct handles")
        #expect(title.masterVolume == MASTER_VOLUME)

        // Thirty frames of frozen title prompt: nothing has played.
        #expect(title.looping == nil, "no music under the title prompt")
        #expect(title.loopStarts == 0)
        #expect(title.oneShotsPlayed == 0)

        // The start key brings the music up: this transition, and only it.
        h.input.press(.space)
        try h.run(frames: 1)
        h.input.release(.space)
        let started = h.audio.stats
        #expect(started.looping == game.music, "the start key starts the music")
        #expect(started.loopStarts == 1)
        #expect(started.oneShotsPlayed == 0)
        #expect(h.isClean, "\(h.lostFrames.prefix(5))")
    }

    @Test("the_music_is_never_layered_across_a_crash_and_replay")
    func theMusicIsNeverLayeredAcrossACrashAndReplay() throws {
        // A replay must not start a second copy of the track: `loopStarts` counts
        // the starts that happened, so it is 1 for a whole session.
        let (game, h) = try driveToGameOver()
        #expect(h.audio.stats.loopStarts == 1)
        #expect(h.audio.stats.oneShotsPlayed == 1, "one impact for the first run")

        // Replay: GameOver to Playing.
        h.input.press(.space)
        try h.run(frames: 1)
        h.input.release(.space)
        #expect(game.state == .playing, "replay resumed play")

        // ...and crash again.
        let framesBefore = h.framesRun
        let ended = try driveUntilOver(game, h, limit: 600)
        try #require(ended, "the replayed run never ended, so this test would prove nothing")

        let stats = h.audio.stats
        #expect(
            stats.loopStarts == 1,
            """
            the music must still be the one loop started at the beginning of the session — a replay that \
            restarts it layers a second copy
            """
        )
        #expect(stats.looping == game.music, "and it is still playing")
        #expect(stats.oneShotsPlayed == 2, "one impact per ended run")

        // Golden: the probe's "second crash after 294 more frames: score 71 best 71
        // travel 95.14734 (0x42be4b70)".
        #expect(h.framesRun - framesBefore == 294)
        #expect(game.score() == 71 && game.bestScore() == 71)
        #expect(bits(game.travel) == 0x42BE_4B70, "travel 95.14734")
        #expect(h.isClean, "\(h.lostFrames.prefix(5))")
    }

    @Test("the_impact_fires_once_per_ended_run_and_not_once_per_frame")
    func theImpactFiresOncePerEndedRunAndNotOncePerFrame() throws {
        // Edge-triggered, not state-triggered: sitting in GameOver must not
        // re-fire the sound.
        let (game, h) = try driveToGameOver()
        let impact = try #require(game.impact, "loaded in init")
        #expect(h.audio.oneShots(of: impact) == 1, "exactly one impact on the crash")

        // Sit in GameOver for a long time (no input, so no replay).
        try h.run(frames: 500)
        #expect(game.state == .gameOver, "still waiting for the replay key")
        #expect(h.audio.oneShots(of: impact) == 1, "500 frames of GameOver must not play the impact again")
        #expect(h.audio.stats.oneShotsPlayed == 1, "and nothing else played either")
    }

    /// Ignored in Rust ("needs a real audio output device; makes sound"): run
    /// with `OFFRAMP_DEVICE_TESTS=1`. Every other audio test runs silent, which
    /// decodes nothing, so only this one would notice a missing or undecodable
    /// committed WAV. It plays at -60 dB.
    @Test(
        "the_committed_sounds_decode_on_a_real_device",
        .enabled(
            if: ProcessInfo.processInfo.environment["OFFRAMP_DEVICE_TESTS"] == "1",
            "needs a real audio output device; set OFFRAMP_DEVICE_TESTS=1"
        )
    )
    func theCommittedSoundsDecodeOnARealDevice() throws {
        let engine = try Engine(
            context: Context(offscreenWidth: 64, height: 48, backend: .null), game: NoGame(),
            options: Engine.Options(audio: .device)
        )
        let audio = engine.audio
        if audio.stats.isSilent {
            print("skipped: no audio output device available")
            return
        }
        let musicPath = AssetRoot.path(MUSIC_ASSET)
        let impactPath = AssetRoot.path(IMPACT_ASSET)
        let music = audio.load(musicPath)
        let impact = audio.load(impactPath)
        #expect(audio.stats.decodeCount == 2, "both committed WAVs decoded (\(musicPath) and \(impactPath))")

        // Quiet enough to be inaudible, loud enough to exercise the real mixer.
        audio.masterVolume = Volume(decibels: -60)
        audio.playLooping(music, volume: Volume(decibels: -60))
        #expect(audio.stats.looping == music, "the music took the loop channel")
        audio.playOnce(impact, volume: Volume(decibels: -60))
        #expect(audio.oneShots(of: impact) == 1, "the impact played once")
        audio.stopLooping()
    }
}
