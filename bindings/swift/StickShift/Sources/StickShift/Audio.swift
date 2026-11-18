// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

import CStickShift

/// A sound an engine loaded, valid for the engine's life (sounds are never
/// unloaded). Only meaningful to the engine that loaded it.
public struct Sound: Hashable, Sendable {
    /// The engine's handle.
    public let id: UInt64

    var c: StickShiftSound { StickShiftSound(id: id) }
}

/// A level, in decibels relative to the sound as recorded.
public struct Volume: Sendable, Equatable {
    /// Decibels: 0 plays a sound as recorded, -60 is silence.
    public var decibels: Float

    /// Creates a volume.
    public init(decibels: Float) {
        self.decibels = decibels
    }

    /// The sound as recorded: 0 dB.
    public static let unchanged = Volume(decibels: STICKSHIFT_VOLUME_UNCHANGED_DB)
    /// Silence: -60 dB.
    public static let silent = Volume(decibels: STICKSHIFT_VOLUME_SILENT_DB)
}

/// The engine's audio counters: what the game asked for, counted when
/// nothing is heard too.
public struct AudioStats: Sendable, Equatable {
    /// Distinct sounds loaded (one per path).
    public var soundCount: Int
    /// Sounds decoded: 0 for a silent engine.
    public var decodeCount: Int
    /// Loops that began playing.
    public var loopStarts: Int
    /// One-shots asked for.
    public var oneShotsPlayed: Int
    /// The level everything plays at.
    public var masterVolume: Volume
    /// The sound on the loop channel.
    public var looping: Sound?
    /// The engine plays to no device.
    public var isSilent: Bool

    init(_ s: StickShiftAudioStats) {
        soundCount = Int(s.sound_count)
        decodeCount = Int(s.decode_count)
        loopStarts = Int(s.loop_starts)
        oneShotsPlayed = Int(s.one_shots_played)
        masterVolume = Volume(decibels: s.master_volume_db)
        looping = s.looping.id == 0 ? nil : Sound(id: s.looping.id)
        isSilent = s.is_silent != 0
    }
}

/// The engine's sound: one-shots over a single loop channel, the engine's
/// `Audio`. A silent engine (``Engine/AudioOutput/silent``) plays nothing but
/// counts everything, so tools and tests see what a game asked for.
@MainActor
public struct Audio {
    let handle: EngineHandle

    /// Load the sound at `path`, a PCM WAV file: the engine's audio decodes
    /// WAV only. Loading a path again returns the same sound. Any other file,
    /// or one that cannot be read or decoded, gives a sound that plays
    /// silence, with a line on stderr, as in Rust.
    public func load(_ path: String) -> Sound {
        var out = StickShiftSound()
        handle.call("Audio.load(_:)") { raw in
            path.withCString { stickshift_audio_load(raw, $0, &out) }
        }
        return Sound(id: out.id)
    }

    /// Play `sound` once at `volume`, over whatever else plays.
    public func playOnce(_ sound: Sound, volume: Volume = .unchanged) {
        handle.call("Audio.playOnce(_:volume:)") { stickshift_audio_play_once($0, sound.c, volume.decibels) }
    }

    /// Loop `sound` at `volume`, replacing what loops now. Asking for the
    /// sound already looping does nothing.
    public func playLooping(_ sound: Sound, volume: Volume = .unchanged) {
        handle.call("Audio.playLooping(_:volume:)") { stickshift_audio_play_looping($0, sound.c, volume.decibels) }
    }

    /// Stop the loop channel.
    public func stopLooping() {
        handle.call("Audio.stopLooping()") { stickshift_audio_stop_looping($0) }
    }

    /// The level everything plays at.
    public var masterVolume: Volume {
        get { stats.masterVolume }
        nonmutating set {
            handle.call("Audio.masterVolume") { stickshift_audio_set_master_volume($0, newValue.decibels) }
        }
    }

    /// The counters.
    public var stats: AudioStats {
        var out = StickShiftAudioStats()
        out.size = UInt32(MemoryLayout<StickShiftAudioStats>.size)
        handle.call("Audio.stats") { stickshift_audio_get_stats($0, &out) }
        return AudioStats(out)
    }

    /// How many one-shots of `sound` the game asked for.
    public func oneShots(of sound: Sound) -> Int {
        var out: UInt32 = 0
        handle.call("Audio.oneShots(of:)") { stickshift_audio_one_shots_of($0, sound.c, &out) }
        return Int(out)
    }

    /// The path `sound` was first loaded by.
    public func path(of sound: Sound) -> String {
        var out: UnsafePointer<CChar>?
        handle.call("Audio.path(of:)") { stickshift_audio_get_path($0, sound.c, &out) }
        return out.map { String(cString: $0) } ?? ""
    }
}
